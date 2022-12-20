use crate::veryl_error::VerylError;
use crate::veryl_grammar_trait::*;
use crate::veryl_walker::VerylWalker;

pub struct Analyzer<'a> {
    text: &'a str,
    pub errors: Vec<VerylError>,
}

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

impl<'a> Analyzer<'a> {
    pub fn new(text: &'a str) -> Self {
        Analyzer {
            text,
            errors: Vec::new(),
        }
    }

    pub fn analyze(&mut self, input: &Veryl) {
        self.veryl(input);
    }
}

impl<'a> VerylWalker for Analyzer<'a> {
    /// Semantic action for non-terminal 'Based'
    fn based(&mut self, arg: &Based) {
        let token = &arg.based_token;
        let text = token.token.token.text();
        let (width, tail) = text.split_once('\'').unwrap();
        let base = &tail[0..1];
        let number = &tail[1..];

        let width: usize = width.replace('_', "").parse().unwrap();
        let number = number.replace('_', "");
        let number = number.trim_start_matches('0');

        match base {
            "b" => {
                if let Some(x) = number.chars().filter(|x| !BINARY_CHARS.contains(&x)).next() {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "binary", &self.text, token,
                    ));
                }
                let actual_width = number.chars().count();
                if actual_width > width {
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            "o" => {
                if let Some(x) = number.chars().filter(|x| !OCTAL_CHARS.contains(&x)).next() {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "octal", &self.text, token,
                    ));
                }
                let mut actual_width = number.chars().count() * 3;
                match number.chars().next() {
                    Some('1') => actual_width -= 2,
                    Some('2') => actual_width -= 1,
                    Some('3') => actual_width -= 1,
                    _ => (),
                }
                if actual_width > width {
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            "d" => {
                if let Some(x) = number
                    .chars()
                    .filter(|x| !DECIMAL_CHARS.contains(&x))
                    .next()
                {
                    self.errors.push(VerylError::invalid_number_character(
                        x, "decimal", &self.text, token,
                    ));
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
                    self.errors
                        .push(VerylError::number_overflow(width, &self.text, token));
                }
            }
            _ => unreachable!(),
        }
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        let if_reset_required = if arg.always_ff_declaration_opt.is_some() {
            if let Some(ref x) = arg.always_ff_declaration_list.first() {
                match &*x.statement {
                    Statement::Statement0(_) => true,
                    Statement::Statement1(_) => true,
                    Statement::Statement2(_) => false,
                    Statement::Statement3(_) => true,
                }
            } else {
                true
            }
        } else {
            false
        };

        if if_reset_required {
            self.errors.push(VerylError::if_reset_required(
                &self.text,
                &arg.always_ff.always_ff_token,
            ));
        }

        let mut if_reset_exist = false;
        for x in &arg.always_ff_declaration_list {
            match &*x.statement {
                Statement::Statement2(_) => if_reset_exist = true,
                _ => (),
            }
        }

        if if_reset_exist && arg.always_ff_declaration_opt.is_none() {
            self.errors.push(VerylError::reset_signal_missing(
                &self.text,
                &arg.always_ff.always_ff_token,
            ));
        }
    }
}
