use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct TokenCollector {
    pub tokens: Vec<Token>,
}

impl VerylWalker for TokenCollector {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.tokens.push(arg.token);
        for x in &arg.comments {
            self.tokens.push(*x);
        }
    }
}
