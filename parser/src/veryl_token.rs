use crate::veryl_grammar_trait::*;
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

        line += text[prev_pos..pos].matches("\n").count();
        prev_pos = pos;

        let location = parol_runtime::lexer::location::Location::with(
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
        let location =
            parol_runtime::lexer::location::Location::with(1, 1, 0, 0, 0, std::path::Path::new(""));
        let token = OwnedToken {
            token: parol_runtime::lexer::Token::with("", 0, location),
        };
        VerylToken { token, comments }
    }
}

token_with_comments!(BasedBinaryToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_tick_b_l_bracket0_minus1xz_x_z_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus1xz_x_z_r_bracket_plus_r_paren_star);
token_with_comments!(BasedOctalToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_tick_o_l_bracket0_minus7xz_x_z_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus7xz_x_z_r_bracket_plus_r_paren_star);
token_with_comments!(BasedDecimalToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_tick_d_l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star);
token_with_comments!(BasedHexToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star_tick_h_l_bracket0_minus9a_minus_f_a_minus_fxz_x_z_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9a_minus_f_a_minus_fxz_x_z_r_bracket_plus_r_paren_star);
token_with_comments!(BaseLessToken, l_bracket0_minus9_r_bracket_plus_l_paren_quest_colon_underscore_l_bracket0_minus9_r_bracket_plus_r_paren_star);

token_with_comments!(PlusToken, plus);
token_with_comments!(MinusToken, minus);
token_with_comments!(StarToken, star);
token_with_comments!(SlashToken, slash);
token_with_comments!(ColonToken, colon);
token_with_comments!(SemicolonToken, semicolon);
token_with_comments!(CommaToken, comma);
token_with_comments!(LParenToken, l_paren);
token_with_comments!(RParenToken, r_paren);
token_with_comments!(LBracketToken, l_bracket);
token_with_comments!(RBracketToken, r_bracket);
token_with_comments!(LBraceToken, l_brace);
token_with_comments!(RBraceToken, r_brace);
token_with_comments!(EquToken, equ);
token_with_comments!(HashToken, hash);

token_with_comments!(LogicToken, logic);
token_with_comments!(BitToken, bit);
token_with_comments!(AlwaysFfToken, always_underscore_ff);
token_with_comments!(AlwaysCombToken, always_underscore_comb);
token_with_comments!(PosedgeToken, posedge);
token_with_comments!(NegedgeToken, negedge);
token_with_comments!(IfToken, r#if);
token_with_comments!(ElseToken, r#else);
token_with_comments!(ParameterToken, parameter);
token_with_comments!(LocalparamToken, localparam);
token_with_comments!(ModuleToken, module);
token_with_comments!(InterfaceToken, interface);
token_with_comments!(InputToken, input);
token_with_comments!(OutputToken, output);
token_with_comments!(InoutToken, inout);
token_with_comments!(ModportToken, modport);
token_with_comments!(U32Token, u32);
token_with_comments!(U64Token, u64);
token_with_comments!(I32Token, i32);
token_with_comments!(I64Token, i64);
token_with_comments!(F32Token, f32);
token_with_comments!(F64Token, f64);

token_with_comments!(IdentifierToken, l_bracket_a_minus_z_a_minus_z_underscore_r_bracket_l_bracket0_minus9a_minus_z_a_minus_z_underscore_r_bracket_star);
