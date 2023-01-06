use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct Finder {
    pub line: usize,
    pub column: usize,
    pub token: Option<Token>,
}

impl Finder {
    pub fn new() -> Self {
        Default::default()
    }
}

impl VerylWalker for Finder {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        if arg.token.line == self.line
            && arg.token.column <= self.column
            && self.column < arg.token.column + arg.token.length
        {
            self.token = Some(arg.token);
        }
    }
}
