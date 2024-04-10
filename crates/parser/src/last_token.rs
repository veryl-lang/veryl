use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct LastToken {
    token: Option<Token>,
}

impl LastToken {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn token(&self) -> &Option<Token> {
        &self.token
    }
}

impl VerylWalker for LastToken {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token = Some(arg.token);
    }
}
