use crate::global_table;
use crate::veryl_grammar_trait::*;
use parol_runtime::miette;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub text: usize,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub pos: usize,
    pub file_path: usize,
}

impl<'t> From<&parol_runtime::lexer::Token<'t>> for Token {
    fn from(x: &parol_runtime::lexer::Token<'t>) -> Self {
        let text = global_table::insert_str(x.text());
        let file_path = global_table::insert_path(&x.location.file_name);
        let source_span: miette::SourceSpan = (&x.location).into();
        Token {
            text,
            line: x.location.line,
            column: x.location.column,
            length: x.location.length,
            pos: source_span.offset(),
            file_path,
        }
    }
}

impl From<&Token> for parol_runtime::miette::SourceSpan {
    fn from(x: &Token) -> Self {
        (x.pos - x.length, x.length).into()
    }
}

impl From<Token> for parol_runtime::miette::SourceSpan {
    fn from(x: Token) -> Self {
        (x.pos - x.length, x.length).into()
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
        let text = global_table::insert_str(text);
        let mut ret = self.clone();
        ret.token.text = text;
        ret.token.length = length;
        ret
    }

    pub fn text(&self) -> String {
        global_table::get_str_value(self.token.text).unwrap()
    }
}

macro_rules! token_with_comments {
    ($x:ident, $y:ident) => {
        impl From<&$x> for VerylToken {
            fn from(x: &$x) -> Self {
                let mut comments = Vec::new();
                if let Some(ref x) = x.comments.comments_opt {
                    let mut tokens = split_comment_token(x.multi_comment.multi_comment);
                    comments.append(&mut tokens)
                }
                VerylToken {
                    token: x.$y.clone(),
                    comments,
                }
            }
        }
    };
}

fn split_comment_token(token: Token) -> Vec<Token> {
    let mut line = token.line;
    let text = global_table::get_str_value(token.text).unwrap();
    let re = Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in re.captures_iter(&text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = cap.end() - pos;

        line += text[prev_pos..pos].matches('\n').count();
        prev_pos = pos;

        let text = global_table::insert_str(&text[pos..pos + length]);
        let token = Token {
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

impl From<&StartToken> for VerylToken {
    fn from(x: &StartToken) -> Self {
        let mut comments = Vec::new();
        if let Some(ref x) = x.comments.comments_opt {
            let mut tokens = split_comment_token(x.multi_comment.multi_comment);
            comments.append(&mut tokens)
        }
        let text = global_table::insert_str("");
        let file_path = global_table::insert_path(std::path::Path::new(""));
        let token = Token {
            text,
            line: 1,
            column: 1,
            length: 0,
            pos: 0,
            file_path,
        };
        VerylToken { token, comments }
    }
}

token_with_comments!(FixedPointToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_dot_l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star);
token_with_comments!(ExponentToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_dot_l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_l_bracket_e_e_r_bracket_l_bracket_plus_minus_r_bracket_quest_l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star);
token_with_comments!(BasedToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_tick_l_bracket_bodh_r_bracket_l_bracket0_minus9a_minus_f_a_minus_fxz_x_z_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9a_minus_f_a_minus_fxz_x_z_r_bracket_plus_r_paren_star);
token_with_comments!(BaseLessToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star);
token_with_comments!(AllBitToken, tick_l_bracket01_r_bracket);

token_with_comments!(ColonToken, colon);
token_with_comments!(CommaToken, comma);
token_with_comments!(DollarToken, dollar);
token_with_comments!(DotDotToken, dot_dot);
token_with_comments!(DotToken, dot);
token_with_comments!(EquToken, equ);
token_with_comments!(HashToken, hash);
token_with_comments!(LBraceToken, l_brace);
token_with_comments!(LBracketToken, l_bracket);
token_with_comments!(LParenToken, l_paren);
token_with_comments!(MinusColonToken, minus_colon);
token_with_comments!(MinusGTToken, minus_g_t);
token_with_comments!(PlusColonToken, plus_colon);
token_with_comments!(RBraceToken, r_brace);
token_with_comments!(RBracketToken, r_bracket);
token_with_comments!(RParenToken, r_paren);
token_with_comments!(SemicolonToken, semicolon);

token_with_comments!(AssignmentOperatorToken, plus_equ_or_minus_equ_or_star_equ_or_slash_equ_or_percent_equ_or_amp_equ_or_or_equ_or_circumflex_equ_or_l_t_l_t_equ_or_g_t_g_t_equ_or_l_t_l_t_l_t_equ_or_g_t_g_t_g_t_equ);
token_with_comments!(CommonOperatorToken, plus_or_minus_or_amp_or_or_or_circumflex_tilde_or_circumflex_or_tilde_circumflex_or_tilde_amp_or_tilde_or);
token_with_comments!(BinaryOperatorToken, star_star_or_star_or_slash_or_percent_or_l_t_l_t_l_t_or_g_t_g_t_g_t_or_l_t_l_t_or_g_t_g_t_or_l_t_equ_or_g_t_equ_or_l_t_or_g_t_or_equ_equ_equ_or_equ_equ_quest_or_bang_equ_equ_or_bang_equ_quest_or_equ_equ_or_bang_equ_or_amp_amp_or_or_or);
token_with_comments!(UnaryOperatorToken, bang_or_tilde);

token_with_comments!(AlwaysCombToken, balways_underscore_comb_b);
token_with_comments!(AlwaysFfToken, balways_underscore_ff_b);
token_with_comments!(AssignToken, bassign_b);
token_with_comments!(AsyncHighToken, basync_underscore_high_b);
token_with_comments!(AsyncLowToken, basync_underscore_low_b);
token_with_comments!(BitToken, bbit_b);
token_with_comments!(ElseToken, belse_b);
token_with_comments!(EnumToken, benum_b);
token_with_comments!(F32Token, bf32_b);
token_with_comments!(F64Token, bf64_b);
token_with_comments!(FunctionToken, bfunction_b);
token_with_comments!(ForToken, bfor_b);
token_with_comments!(I32Token, bi32_b);
token_with_comments!(I64Token, bi64_b);
token_with_comments!(IfToken, bif_b);
token_with_comments!(IfResetToken, bif_underscore_reset_b);
token_with_comments!(InoutToken, binout_b);
token_with_comments!(InputToken, binput_b);
token_with_comments!(InstToken, binst_b);
token_with_comments!(InterfaceToken, binterface_b);
token_with_comments!(InToken, bin_b);
token_with_comments!(LetToken, blet_b);
token_with_comments!(LocalparamToken, blocalparam_b);
token_with_comments!(LogicToken, blogic_b);
token_with_comments!(ModportToken, bmodport_b);
token_with_comments!(ModuleToken, bmodule_b);
token_with_comments!(NegedgeToken, bnegedge_b);
token_with_comments!(OutputToken, boutput_b);
token_with_comments!(ParameterToken, bparameter_b);
token_with_comments!(PosedgeToken, bposedge_b);
token_with_comments!(RefToken, bref_b);
token_with_comments!(ReturnToken, breturn_b);
token_with_comments!(StepToken, bstep_b);
token_with_comments!(StructToken, bstruct_b);
token_with_comments!(SyncHighToken, bsync_underscore_high_b);
token_with_comments!(SyncLowToken, bsync_underscore_low_b);
token_with_comments!(U32Token, bu32_b);
token_with_comments!(U64Token, bu64_b);

token_with_comments!(IdentifierToken, l_bracket_a_minus_z_a_minus_z_underscore_r_bracket_l_bracket0_minus9a_minus_z_a_minus_z_underscore_r_bracket_star);
