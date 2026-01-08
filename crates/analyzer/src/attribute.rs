use std::cell::RefCell;
use std::fmt;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_token::Token;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribute {
    Ifdef(StrId),
    Ifndef(StrId),
    Elsif(StrId, Vec<StrId>, Vec<StrId>),
    Else(Vec<StrId>, Vec<StrId>),
    Sv(StrId),
    Allow(AllowItem),
    EnumEncoding(EnumEncodingItem),
    EnumMemberPrefix(StrId),
    Test(Token, Option<StrId>),
    CondType(CondTypeItem),
    Align(Vec<AlignItem>),
    Format(Vec<FormatItem>),
    Expand(Vec<ExpandItem>),
}

impl Attribute {
    pub fn is_align(&self, item: AlignItem) -> bool {
        if let Attribute::Align(x) = self {
            x.contains(&item)
        } else {
            false
        }
    }

    pub fn is_format(&self, item: FormatItem) -> bool {
        if let Attribute::Format(x) = self {
            x.contains(&item)
        } else {
            false
        }
    }

    pub fn is_ifdef(&self) -> bool {
        matches!(
            self,
            Attribute::Ifdef(_)
                | Attribute::Ifndef(_)
                | Attribute::Elsif(_, _, _)
                | Attribute::Else(_, _)
        )
    }

    pub fn is_expand(&self, item: ExpandItem) -> bool {
        if let Attribute::Expand(x) = self {
            x.contains(&item)
        } else {
            false
        }
    }
}

impl fmt::Display for Attribute {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            Attribute::Ifdef(x) => format!("ifdef({x})"),
            Attribute::Ifndef(x) => format!("ifndef({x})"),
            Attribute::Elsif(x, _, _) => format!("elsif({x})"),
            Attribute::Else(_, _) => String::from("else"),
            Attribute::Sv(x) => format!("sv(\"{x}\")"),
            Attribute::Allow(x) => format!("allow({x})"),
            Attribute::EnumEncoding(x) => format!("enum_encoding({x})"),
            Attribute::EnumMemberPrefix(x) => format!("enum_member_prefix({x})"),
            Attribute::Test(x, _) => format!("test({})", x.text),
            Attribute::CondType(x) => format!("cond_type({x})"),
            Attribute::Align(x) => {
                let mut arg = String::new();
                for x in x {
                    arg.push_str(&format!("{x}, "));
                }
                format!("align({arg})")
            }
            Attribute::Format(x) => {
                let mut arg = String::new();
                for x in x {
                    arg.push_str(&format!("{x}, "));
                }
                format!("format({arg})")
            }
            Attribute::Expand(x) => {
                let mut arg = String::new();
                for x in x {
                    arg.push_str(&format!("{x}, "));
                }
                format!("expand({arg})")
            }
        };
        text.fmt(f)
    }
}

#[derive(Clone, Debug)]
pub enum AttributeError {
    UnknownAttribute,
    MismatchArgs(String),
}

fn get_arg_ident(
    args: &Option<veryl_parser::veryl_grammar_trait::AttributeOpt>,
    pos: usize,
) -> Option<Token> {
    use veryl_parser::veryl_grammar_trait as g;

    if let Some(x) = args {
        let args: Vec<_> = x.attribute_list.as_ref().into();
        if args.len() <= pos {
            None
        } else if let g::AttributeItem::Identifier(x) = args[pos] {
            Some(x.identifier.identifier_token.token)
        } else {
            None
        }
    } else {
        None
    }
}

fn get_arg_string(
    args: &Option<veryl_parser::veryl_grammar_trait::AttributeOpt>,
    pos: usize,
) -> Option<Token> {
    use veryl_parser::veryl_grammar_trait as g;

    if let Some(x) = args {
        let args: Vec<_> = x.attribute_list.as_ref().into();
        if args.len() <= pos {
            None
        } else if let g::AttributeItem::StringLiteral(x) = &args[pos] {
            Some(x.string_literal.string_literal_token.token)
        } else {
            None
        }
    } else {
        None
    }
}

fn get_args_ident(args: &Option<veryl_parser::veryl_grammar_trait::AttributeOpt>) -> Vec<Token> {
    use veryl_parser::veryl_grammar_trait as g;

    let mut ret = Vec::new();

    if let Some(x) = args {
        let args: Vec<_> = x.attribute_list.as_ref().into();
        for arg in args {
            if let g::AttributeItem::Identifier(x) = arg {
                ret.push(x.identifier.identifier_token.token);
            }
        }
    }
    ret
}

struct Pattern {
    pub ifdef: StrId,
    pub ifndef: StrId,
    pub elsif: StrId,
    pub r#else: StrId,
    pub sv: StrId,
    pub allow: StrId,
    pub missing_port: StrId,
    pub missing_reset_statement: StrId,
    pub unused_variable: StrId,
    pub unassign_variable: StrId,
    pub enum_encoding: StrId,
    pub sequential: StrId,
    pub onehot: StrId,
    pub gray: StrId,
    pub enum_member_prefix: StrId,
    pub test: StrId,
    pub cond_type: StrId,
    pub unique: StrId,
    pub unique0: StrId,
    pub priority: StrId,
    pub none: StrId,
    pub align: StrId,
    pub number: StrId,
    pub identifier: StrId,
    pub fmt: StrId,
    pub compact: StrId,
    pub skip: StrId,
    pub expand: StrId,
    pub modport: StrId,
}

impl Pattern {
    fn new() -> Self {
        Self {
            ifdef: resource_table::insert_str("ifdef"),
            ifndef: resource_table::insert_str("ifndef"),
            elsif: resource_table::insert_str("elsif"),
            r#else: resource_table::insert_str("else"),
            sv: resource_table::insert_str("sv"),
            allow: resource_table::insert_str("allow"),
            missing_port: resource_table::insert_str("missing_port"),
            missing_reset_statement: resource_table::insert_str("missing_reset_statement"),
            unused_variable: resource_table::insert_str("unused_variable"),
            unassign_variable: resource_table::insert_str("unassign_variable"),
            enum_encoding: resource_table::insert_str("enum_encoding"),
            sequential: resource_table::insert_str("sequential"),
            onehot: resource_table::insert_str("onehot"),
            gray: resource_table::insert_str("gray"),
            enum_member_prefix: resource_table::insert_str("enum_member_prefix"),
            test: resource_table::insert_str("test"),
            cond_type: resource_table::insert_str("cond_type"),
            unique: resource_table::insert_str("unique"),
            unique0: resource_table::insert_str("unique0"),
            priority: resource_table::insert_str("priority"),
            none: resource_table::insert_str("none"),
            align: resource_table::insert_str("align"),
            number: resource_table::insert_str("number"),
            identifier: resource_table::insert_str("identifier"),
            fmt: resource_table::insert_str("fmt"),
            compact: resource_table::insert_str("compact"),
            skip: resource_table::insert_str("skip"),
            expand: resource_table::insert_str("expand"),
            modport: resource_table::insert_str("modport"),
        }
    }
}

thread_local!(static PAT: RefCell<Pattern> = RefCell::new(Pattern::new()));

impl TryFrom<&veryl_parser::veryl_grammar_trait::Attribute> for Attribute {
    type Error = AttributeError;

    fn try_from(value: &veryl_parser::veryl_grammar_trait::Attribute) -> Result<Self, Self::Error> {
        PAT.with_borrow(|pat| match value.identifier.identifier_token.token.text {
            x if x == pat.ifdef || x == pat.ifndef || x == pat.elsif || x == pat.r#else => {
                let arg = get_arg_ident(&value.attribute_opt, 0);

                if let Some(arg) = arg {
                    match x {
                        x if x == pat.ifdef => Ok(Attribute::Ifdef(arg.text)),
                        x if x == pat.ifndef => Ok(Attribute::Ifndef(arg.text)),
                        x if x == pat.elsif => {
                            Ok(Attribute::Elsif(arg.text, Vec::new(), Vec::new()))
                        }
                        x if x == pat.r#else => {
                            Err(AttributeError::MismatchArgs("no argument".to_string()))
                        }
                        _ => unreachable!(),
                    }
                } else if x == pat.r#else {
                    Ok(Attribute::Else(Vec::new(), Vec::new()))
                } else {
                    Err(AttributeError::MismatchArgs(
                        "single identifier".to_string(),
                    ))
                }
            }
            x if x == pat.sv => {
                let arg = get_arg_string(&value.attribute_opt, 0);

                if let Some(arg) = arg {
                    Ok(Attribute::Sv(arg.text))
                } else {
                    Err(AttributeError::MismatchArgs("single string".to_string()))
                }
            }
            x if x == pat.allow => {
                let arg = get_arg_ident(&value.attribute_opt, 0);

                let err =
                    AttributeError::MismatchArgs(format!("rule: ({})", AllowItem::available()));

                if let Some(arg) = arg {
                    match arg.text {
                        x if x == pat.missing_port => Ok(Attribute::Allow(AllowItem::MissingPort)),
                        x if x == pat.missing_reset_statement => {
                            Ok(Attribute::Allow(AllowItem::MissingResetStatement))
                        }
                        x if x == pat.unused_variable => {
                            Ok(Attribute::Allow(AllowItem::UnusedVariable))
                        }
                        x if x == pat.unassign_variable => {
                            Ok(Attribute::Allow(AllowItem::UnassignVariable))
                        }
                        _ => Err(err),
                    }
                } else {
                    Err(err)
                }
            }
            x if x == pat.enum_encoding => {
                let arg = get_arg_ident(&value.attribute_opt, 0);

                let err = AttributeError::MismatchArgs(format!(
                    "encoding type: ({})",
                    EnumEncodingItem::available()
                ));

                if let Some(arg) = arg {
                    match arg.text {
                        x if x == pat.sequential => {
                            Ok(Attribute::EnumEncoding(EnumEncodingItem::Sequential))
                        }
                        x if x == pat.onehot => {
                            Ok(Attribute::EnumEncoding(EnumEncodingItem::OneHot))
                        }
                        x if x == pat.gray => Ok(Attribute::EnumEncoding(EnumEncodingItem::Gray)),
                        _ => Err(err),
                    }
                } else {
                    Err(err)
                }
            }
            x if x == pat.enum_member_prefix => {
                let arg = get_arg_ident(&value.attribute_opt, 0);

                if let Some(arg) = arg {
                    Ok(Attribute::EnumMemberPrefix(arg.text))
                } else {
                    Err(AttributeError::MismatchArgs(
                        "single identifier".to_string(),
                    ))
                }
            }
            x if x == pat.test => {
                let arg = get_arg_ident(&value.attribute_opt, 0);
                let top = get_arg_ident(&value.attribute_opt, 1);

                if let Some(arg) = arg {
                    Ok(Attribute::Test(arg, top.map(|x| x.text)))
                } else {
                    Err(AttributeError::MismatchArgs(
                        "single identifier".to_string(),
                    ))
                }
            }
            x if x == pat.cond_type => {
                let arg = get_arg_ident(&value.attribute_opt, 0);

                let err = AttributeError::MismatchArgs(format!(
                    "condition type: ({})",
                    CondTypeItem::available()
                ));

                if let Some(arg) = arg {
                    match arg.text {
                        x if x == pat.unique => Ok(Attribute::CondType(CondTypeItem::Unique)),
                        x if x == pat.unique0 => Ok(Attribute::CondType(CondTypeItem::Unique0)),
                        x if x == pat.priority => Ok(Attribute::CondType(CondTypeItem::Priority)),
                        x if x == pat.none => Ok(Attribute::CondType(CondTypeItem::None)),
                        _ => Err(err),
                    }
                } else {
                    Err(err)
                }
            }
            x if x == pat.align => {
                let args = get_args_ident(&value.attribute_opt);
                let mut items = Vec::new();

                let err = AttributeError::MismatchArgs(format!(
                    "align type: ({})",
                    AlignItem::available()
                ));

                for arg in &args {
                    match arg.text {
                        x if x == pat.number => items.push(AlignItem::Number),
                        x if x == pat.identifier => items.push(AlignItem::Identifier),
                        _ => return Err(err),
                    }
                }

                if args.is_empty() {
                    Err(err)
                } else {
                    Ok(Attribute::Align(items))
                }
            }
            x if x == pat.fmt => {
                let args = get_args_ident(&value.attribute_opt);
                let mut items = Vec::new();

                let err = AttributeError::MismatchArgs(format!(
                    "format type: ({})",
                    FormatItem::available()
                ));

                for arg in &args {
                    match arg.text {
                        x if x == pat.compact => items.push(FormatItem::Compact),
                        x if x == pat.skip => items.push(FormatItem::Skip),
                        _ => return Err(err),
                    }
                }

                if args.is_empty() {
                    Err(err)
                } else {
                    Ok(Attribute::Format(items))
                }
            }
            x if x == pat.expand => {
                let args = get_args_ident(&value.attribute_opt);
                let mut items = Vec::new();

                let err = AttributeError::MismatchArgs(format!(
                    "expand type: ({})",
                    ExpandItem::available()
                ));

                for arg in &args {
                    match arg.text {
                        x if x == pat.modport => items.push(ExpandItem::Modport),
                        _ => return Err(err),
                    }
                }

                if args.is_empty() {
                    Err(err)
                } else {
                    Ok(Attribute::Expand(items))
                }
            }
            _ => Err(AttributeError::UnknownAttribute),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum AllowItem {
    MissingPort,
    MissingResetStatement,
    UnusedVariable,
    UnassignVariable,
}

impl AllowItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for AllowItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            AllowItem::MissingPort => "missing_port",
            AllowItem::MissingResetStatement => "missing_reset_statement",
            AllowItem::UnusedVariable => "unused_variable",
            AllowItem::UnassignVariable => "unassign_variable",
        };
        text.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, EnumIter)]
pub enum EnumEncodingItem {
    #[default]
    Sequential,
    OneHot,
    Gray,
}

impl EnumEncodingItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for EnumEncodingItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            EnumEncodingItem::Sequential => "sequential",
            EnumEncodingItem::OneHot => "one_hot",
            EnumEncodingItem::Gray => "gray",
        };
        text.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum CondTypeItem {
    Unique,
    Unique0,
    Priority,
    None,
}

impl CondTypeItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for CondTypeItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            CondTypeItem::Unique => "unique",
            CondTypeItem::Unique0 => "unique0",
            CondTypeItem::Priority => "priority",
            CondTypeItem::None => "none",
        };
        text.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum AlignItem {
    Number,
    Identifier,
}

impl AlignItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for AlignItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            AlignItem::Number => "number",
            AlignItem::Identifier => "identifier",
        };
        text.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum FormatItem {
    Compact,
    Skip,
}

impl FormatItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for FormatItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            FormatItem::Compact => "compact",
            FormatItem::Skip => "skip",
        };
        text.fmt(f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter)]
pub enum ExpandItem {
    Modport,
}

impl ExpandItem {
    pub fn available() -> String {
        let mut ret = String::new();
        for (i, x) in Self::iter().enumerate() {
            if i != 0 {
                ret.push('|');
            }
            ret.push_str(&format!("{x}"));
        }
        ret
    }
}

impl fmt::Display for ExpandItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            ExpandItem::Modport => "modport",
        };
        text.fmt(f)
    }
}
