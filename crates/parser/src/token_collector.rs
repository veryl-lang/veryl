use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct TokenCollector {
    pub tokens: Vec<Token>,
    include_comments: bool,
}

impl TokenCollector {
    pub fn new(include_comments: bool) -> Self {
        Self {
            tokens: Vec::new(),
            include_comments,
        }
    }
}

impl VerylWalker for TokenCollector {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.tokens.push(arg.token);
        if self.include_comments {
            for x in &arg.comments {
                self.tokens.push(*x);
            }
        }
    }
}
