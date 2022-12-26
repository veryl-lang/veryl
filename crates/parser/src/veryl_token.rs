use crate::veryl_grammar_trait::*;
use parol_runtime::lexer::{Location, Token};
use regex::Regex;

#[derive(Debug, Clone)]
pub struct OwnedToken {
    pub token: parol_runtime::lexer::Token<'static>,
}

impl<'t> From<&parol_runtime::lexer::Token<'t>> for OwnedToken {
    fn from(x: &parol_runtime::lexer::Token<'t>) -> Self {
        OwnedToken {
            token: x.clone().into_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerylToken {
    pub token: OwnedToken,
    pub comments: Vec<OwnedToken>,
}

impl VerylToken {
    pub fn replace(&self, text: &str) -> Self {
        let mut location = self.token.token.location.clone();
        location.length = text.len();
        let token = parol_runtime::lexer::Token::with(
            text.to_owned(),
            self.token.token.token_type,
            location,
        );
        let mut ret = self.clone();
        ret.token.token = token;
        ret
    }

    pub fn text(&self) -> &str {
        self.token.token.text()
    }

    pub fn location(&self) -> &Location {
        &self.token.token.location
    }

    pub fn parol_token(&self) -> &Token {
        &self.token.token
    }
}

macro_rules! token_with_comments {
    ($x:ident, $y:ident) => {
        impl From<&$x> for VerylToken {
            fn from(x: &$x) -> Self {
                let mut comments = Vec::new();
                if let Some(ref x) = x.comments.comments_opt {
                    let mut tokens = split_comment_token(x.multi_comment.multi_comment.clone());
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

fn split_comment_token(token: OwnedToken) -> Vec<OwnedToken> {
    let mut line = token.token.location.line;
    let text = token.token.text();
    let re = Regex::new(r"((?://.*(?:\r\n|\r|\n|$))|(?:(?ms)/\u{2a}.*?\u{2a}/))").unwrap();

    let mut prev_pos = 0;
    let mut ret = Vec::new();
    for cap in re.captures_iter(text) {
        let cap = cap.get(0).unwrap();
        let pos = cap.start();
        let length = cap.end() - pos;

        line += text[prev_pos..pos].matches('\n').count();
        prev_pos = pos;

        let location = Location::with(
            line,
            0, // column is not used
            length,
            0, // start_pos is not used
            0, // pos is not used
            token.token.location.file_name.clone(),
        );

        let text = String::from(&text[pos..pos + length]);
        let token = parol_runtime::lexer::Token::with(text, 0, location);
        let token = OwnedToken { token };
        ret.push(token);
    }
    ret
}

impl From<&StartToken> for VerylToken {
    fn from(x: &StartToken) -> Self {
        let mut comments = Vec::new();
        if let Some(ref x) = x.comments.comments_opt {
            let mut tokens = split_comment_token(x.multi_comment.multi_comment.clone());
            comments.append(&mut tokens)
        }
        let location = Location::with(1, 1, 0, 0, 0, std::path::Path::new(""));
        let token = OwnedToken {
            token: Token::with("", 0, location),
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
token_with_comments!(DotDotToken, dot_dot);
token_with_comments!(EquToken, equ);
token_with_comments!(HashToken, hash);
token_with_comments!(LBraceToken, l_brace);
token_with_comments!(LBracketToken, l_bracket);
token_with_comments!(LParenToken, l_paren);
token_with_comments!(MinusGTToken, minus_g_t);
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
