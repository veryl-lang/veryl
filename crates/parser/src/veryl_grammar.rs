use crate::veryl_grammar_trait::*;
use parol_runtime::miette::Result;
use std::fmt::{Debug, Display, Error, Formatter};

#[derive(Debug, Default)]
pub struct VerylGrammar {
    pub veryl: Option<Veryl>,
}

impl VerylGrammar {
    pub fn new() -> Self {
        VerylGrammar::default()
    }
}

impl Display for Veryl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), Error> {
        write!(f, "{:?}", self)
    }
}

impl Display for VerylGrammar {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), Error> {
        match &self.veryl {
            Some(veryl) => writeln!(f, "{}", veryl),
            None => write!(f, "No parse result"),
        }
    }
}

impl VerylGrammarTrait for VerylGrammar {
    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl) -> Result<()> {
        self.veryl = Some(arg.clone());
        Ok(())
    }
}
