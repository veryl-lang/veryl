use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::veryl_grammar_trait::*;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub id: TokenId,
    pub text: StrId,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub pos: usize,
    pub file_path: PathId,
}

impl<'t> TryFrom<&parol_runtime::lexer::Token<'t>> for Token {
    type Error = anyhow::Error;
    fn try_from(x: &parol_runtime::lexer::Token<'t>) -> Result<Self, anyhow::Error> {
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(x.text());
        let pos = x.location.scanner_switch_pos + x.location.offset - x.location.length;
        let file_path = resource_table::insert_path(&x.location.file_name);
        Ok(Token {
            id,
            text,
            line: x.location.start_line,
            column: x.location.start_column,
            length: x.location.length,
            pos,
            file_path,
        })
    }
}

impl From<&Token> for miette::SourceSpan {
    fn from(x: &Token) -> Self {
        (x.pos, x.length).into()
    }
}

impl From<Token> for miette::SourceSpan {
    fn from(x: Token) -> Self {
        (x.pos, x.length).into()
    }
}

#[derive(Debug, Clone)]
pub struct VerylToken {
    pub token: Token,
    pub comments: Vec<Token>,
}

impl VerylToken {
    pub fn replace(&self, text: &str) -> Self {
        let length = text.len();
        let text = resource_table::insert_str(text);
        let mut ret = self.clone();
        ret.token.text = text;
        ret.token.length = length;
        ret
    }

    pub fn text(&self) -> String {
        resource_table::get_str_value(self.token.text).unwrap()
    }
}

macro_rules! token_with_comments {
    ($x:ident, $y:ident, $z:ident) => {
        impl TryFrom<&$x> for VerylToken {
            type Error = anyhow::Error;

            fn try_from(x: &$x) -> Result<Self, anyhow::Error> {
                let mut comments = Vec::new();
                if let Some(ref x) = x.comments.comments_opt {
                    let mut tokens = split_comment_token(x.comments_term.comments_term);
                    comments.append(&mut tokens)
                }
                Ok(VerylToken {
                    token: x.$z.clone(),
                    comments,
                })
            }
        }
        impl TryFrom<&$y> for Token {
            type Error = anyhow::Error;

            fn try_from(x: &$y) -> Result<Self, anyhow::Error> {
                Ok(Token {
                    id: x.$z.id,
                    text: x.$z.text,
                    line: x.$z.line,
                    column: x.$z.column,
                    length: x.$z.length,
                    pos: x.$z.pos,
                    file_path: x.$z.file_path,
                })
            }
        }
    };
}

fn split_comment_token(token: Token) -> Vec<Token> {
    let mut line = token.line;
    let text = resource_table::get_str_value(token.text).unwrap();
    let re = Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in re.captures_iter(&text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = cap.end() - pos;

        line += text[prev_pos..pos].matches('\n').count();
        prev_pos = pos;

        let id = resource_table::new_token_id();
        let text = resource_table::insert_str(&text[pos..pos + length]);
        let token = Token {
            id,
            text,
            line,
            column: 0,
            length,
            pos: pos + length,
            file_path: token.file_path,
        };
        ret.push(token);
    }
    ret
}

impl TryFrom<&StartToken> for VerylToken {
    type Error = anyhow::Error;

    fn try_from(x: &StartToken) -> Result<Self, anyhow::Error> {
        let mut comments = Vec::new();
        if let Some(ref x) = x.comments.comments_opt {
            let mut tokens = split_comment_token(x.comments_term.comments_term);
            comments.append(&mut tokens)
        }
        let id = resource_table::new_token_id();
        let text = resource_table::insert_str("");
        let file_path = resource_table::insert_path(std::path::Path::new(""));
        let token = Token {
            id,
            text,
            line: 1,
            column: 1,
            length: 0,
            pos: 0,
            file_path,
        };
        Ok(VerylToken { token, comments })
    }
}

token_with_comments!(StringToken, StringTerm, string_term);

token_with_comments!(FixedPointToken, FixedPointTerm, fixed_point_term);
token_with_comments!(ExponentToken, ExponentTerm, exponent_term);
token_with_comments!(BasedToken, BasedTerm, based_term);
token_with_comments!(BaseLessToken, BaseLessTerm, base_less_term);
token_with_comments!(AllBitToken, AllBitTerm, all_bit_term);

token_with_comments!(ColonToken, ColonTerm, colon_term);
token_with_comments!(ColonColonToken, ColonColonTerm, colon_colon_term);
token_with_comments!(CommaToken, CommaTerm, comma_term);
token_with_comments!(DollarToken, DollarTerm, dollar_term);
token_with_comments!(DotDotToken, DotDotTerm, dot_dot_term);
token_with_comments!(DotToken, DotTerm, dot_term);
token_with_comments!(EquToken, EquTerm, equ_term);
token_with_comments!(HashToken, HashTerm, hash_term);
token_with_comments!(LBraceToken, LBraceTerm, l_brace_term);
token_with_comments!(LBracketToken, LBracketTerm, l_bracket_term);
token_with_comments!(LParenToken, LParenTerm, l_paren_term);
token_with_comments!(MinusColonToken, MinusColonTerm, minus_colon_term);
token_with_comments!(MinusGTToken, MinusGTTerm, minus_g_t_term);
token_with_comments!(PlusColonToken, PlusColonTerm, plus_colon_term);
token_with_comments!(RBraceToken, RBraceTerm, r_brace_term);
token_with_comments!(RBracketToken, RBracketTerm, r_bracket_term);
token_with_comments!(RParenToken, RParenTerm, r_paren_term);
token_with_comments!(SemicolonToken, SemicolonTerm, semicolon_term);
token_with_comments!(StarToken, StarTerm, star_term);

token_with_comments!(
    AssignmentOperatorToken,
    AssignmentOperatorTerm,
    assignment_operator_term
);
token_with_comments!(Operator01Token, Operator01Term, operator01_term);
token_with_comments!(Operator02Token, Operator02Term, operator02_term);
token_with_comments!(Operator03Token, Operator03Term, operator03_term);
token_with_comments!(Operator04Token, Operator04Term, operator04_term);
token_with_comments!(Operator05Token, Operator05Term, operator05_term);
token_with_comments!(Operator06Token, Operator06Term, operator06_term);
token_with_comments!(Operator07Token, Operator07Term, operator07_term);
token_with_comments!(Operator08Token, Operator08Term, operator08_term);
token_with_comments!(Operator09Token, Operator09Term, operator09_term);
token_with_comments!(Operator10Token, Operator10Term, operator10_term);
token_with_comments!(Operator11Token, Operator11Term, operator11_term);
token_with_comments!(UnaryOperatorToken, UnaryOperatorTerm, unary_operator_term);

token_with_comments!(AlwaysCombToken, AlwaysCombTerm, always_comb_term);
token_with_comments!(AlwaysFfToken, AlwaysFfTerm, always_ff_term);
token_with_comments!(AsToken, AsTerm, as_term);
token_with_comments!(AssignToken, AssignTerm, assign_term);
token_with_comments!(AsyncHighToken, AsyncHighTerm, async_high_term);
token_with_comments!(AsyncLowToken, AsyncLowTerm, async_low_term);
token_with_comments!(BitToken, BitTerm, bit_term);
token_with_comments!(CaseToken, CaseTerm, case_term);
token_with_comments!(DefaultToken, DefaultTerm, default_term);
token_with_comments!(ElseToken, ElseTerm, else_term);
token_with_comments!(EnumToken, EnumTerm, enum_term);
token_with_comments!(ExportToken, ExportTerm, export_term);
token_with_comments!(F32Token, F32Term, f32_term);
token_with_comments!(F64Token, F64Term, f64_term);
token_with_comments!(FunctionToken, FunctionTerm, function_term);
token_with_comments!(ForToken, ForTerm, for_term);
token_with_comments!(I32Token, I32Term, i32_term);
token_with_comments!(I64Token, I64Term, i64_term);
token_with_comments!(IfToken, IfTerm, if_term);
token_with_comments!(IfResetToken, IfResetTerm, if_reset_term);
token_with_comments!(ImportToken, ImportTerm, import_term);
token_with_comments!(InoutToken, InoutTerm, inout_term);
token_with_comments!(InputToken, InputTerm, input_term);
token_with_comments!(InstToken, InstTerm, inst_term);
token_with_comments!(InterfaceToken, InterfaceTerm, interface_term);
token_with_comments!(InToken, InTerm, in_term);
token_with_comments!(LocalparamToken, LocalparamTerm, localparam_term);
token_with_comments!(LogicToken, LogicTerm, logic_term);
token_with_comments!(ModportToken, ModportTerm, modport_term);
token_with_comments!(ModuleToken, ModuleTerm, module_term);
token_with_comments!(NegedgeToken, NegedgeTerm, negedge_term);
token_with_comments!(OutputToken, OutputTerm, output_term);
token_with_comments!(PackageToken, PackageTerm, package_term);
token_with_comments!(ParameterToken, ParameterTerm, parameter_term);
token_with_comments!(PosedgeToken, PosedgeTerm, posedge_term);
token_with_comments!(RefToken, RefTerm, ref_term);
token_with_comments!(RepeatToken, RepeatTerm, repeat_term);
token_with_comments!(ReturnToken, ReturnTerm, return_term);
token_with_comments!(SignedToken, SignedTerm, signed_term);
token_with_comments!(StepToken, StepTerm, step_term);
token_with_comments!(StructToken, StructTerm, struct_term);
token_with_comments!(SyncHighToken, SyncHighTerm, sync_high_term);
token_with_comments!(SyncLowToken, SyncLowTerm, sync_low_term);
token_with_comments!(TriToken, TriTerm, tri_term);
token_with_comments!(U32Token, U32Term, u32_term);
token_with_comments!(U64Token, U64Term, u64_term);
token_with_comments!(VarToken, VarTerm, var_term);

token_with_comments!(IdentifierToken, IdentifierTerm, identifier_term);
