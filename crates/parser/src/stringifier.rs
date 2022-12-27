use crate::veryl_token::VerylToken;
use crate::veryl_walker::VerylWalker;

#[derive(Default)]
pub struct Stringifier {
    string: String,
}

impl Stringifier {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }
}

impl VerylWalker for Stringifier {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.string.push_str(arg.text());
    }
}
