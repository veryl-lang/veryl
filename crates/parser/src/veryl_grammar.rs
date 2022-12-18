use crate::veryl_error::VerylError;
use crate::veryl_grammar_trait::*;
use parol_runtime::lexer::Token;
#[allow(unused_imports)]
use parol_runtime::miette::{bail, IntoDiagnostic, Result};
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

    /// Semantic action for non-terminal 'Based'
    fn based(&mut self, arg: &Based) -> Result<()> {
        check_based(&arg.based_token.token.token)?;
        Ok(())
    }
}

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

fn check_based(token: &Token) -> Result<()> {
    let text = token.text();
    let (width, tail) = text.split_once('\'').unwrap();
    let base = &tail[0..1];
    let number = &tail[1..];

    let width: usize = width.replace('_', "").parse().unwrap();
    let number = number.replace('_', "");
    let number = number.trim_start_matches('0');

    match base {
        "b" => {
            if let Some(x) = number.chars().filter(|x| !BINARY_CHARS.contains(&x)).next() {
                let msg = format!("binary number can't contain {}", x);
                return Err(VerylError::semantic_error(msg, token).into());
            }
            let actual_width = number.chars().count();
            if actual_width > width {
                let msg = format!("number is over the maximum size of {} bits", width);
                return Err(VerylError::semantic_error(msg, token).into());
            }
        }
        "o" => {
            if let Some(x) = number.chars().filter(|x| !OCTAL_CHARS.contains(&x)).next() {
                let msg = format!("octal number can't contain {}", x);
                return Err(VerylError::semantic_error(msg, token).into());
            }
            let mut actual_width = number.chars().count() * 3;
            match number.chars().next() {
                Some('1') => actual_width -= 2,
                Some('2') => actual_width -= 1,
                Some('3') => actual_width -= 1,
                _ => (),
            }
            if actual_width > width {
                let msg = format!("number is over the maximum size of {} bits", width);
                return Err(VerylError::semantic_error(msg, token).into());
            }
        }
        "d" => {
            if let Some(x) = number
                .chars()
                .filter(|x| !DECIMAL_CHARS.contains(&x))
                .next()
            {
                let msg = format!("decimal number can't contain {}", x);
                return Err(VerylError::semantic_error(msg, token).into());
            }
        }
        "h" => {
            let mut actual_width = number.chars().count() * 4;
            match number.chars().next() {
                Some('1') => actual_width -= 3,
                Some('2') => actual_width -= 2,
                Some('3') => actual_width -= 2,
                Some('4') => actual_width -= 1,
                Some('5') => actual_width -= 1,
                Some('6') => actual_width -= 1,
                Some('7') => actual_width -= 1,
                _ => (),
            }
            if actual_width > width {
                let msg = format!("number is over the maximum size of {} bits", width);
                return Err(VerylError::semantic_error(msg, token).into());
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}
