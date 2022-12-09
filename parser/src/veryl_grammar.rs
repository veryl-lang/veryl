use crate::veryl_grammar_trait::*;
#[allow(unused_imports)]
use parol_runtime::miette::Result;
use std::fmt::{Debug, Display, Error, Formatter};

#[derive(Debug, Default)]
pub struct VerylGrammar<'t> {
    pub veryl: Option<Veryl<'t>>,
}

impl VerylGrammar<'_> {
    pub fn new() -> Self {
        VerylGrammar::default()
    }
}

impl Display for Veryl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), Error> {
        write!(f, "{:?}", self)
    }
}

impl Display for VerylGrammar<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), Error> {
        match &self.veryl {
            Some(veryl) => writeln!(f, "{}", veryl),
            None => write!(f, "No parse result"),
        }
    }
}

impl<'t> VerylGrammarTrait<'t> for VerylGrammar<'t> {
    /// Semantic action for non-terminal 'Veryl'
    fn veryl(&mut self, arg: &Veryl<'t>) -> Result<()> {
        self.veryl = Some(arg.clone());
        Ok(())
    }
}
