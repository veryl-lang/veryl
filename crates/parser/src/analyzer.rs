use crate::veryl_error::VerylError;
use crate::veryl_grammar_trait::*;
use crate::veryl_walker::VerylWalker;

pub struct Analyzer<'a> {
    pub errors: Vec<VerylError>,
    text: &'a str,
    in_always_ff: bool,
    in_always_comb: bool,
    in_function: bool,
}

const BINARY_CHARS: [char; 6] = ['0', '1', 'x', 'z', 'X', 'Z'];
const OCTAL_CHARS: [char; 12] = ['0', '1', '2', '3', '4', '5', '6', '7', 'x', 'z', 'X', 'Z'];
const DECIMAL_CHARS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

impl<'a> Analyzer<'a> {
    pub fn new(text: &'a str) -> Self {
        Analyzer {
            errors: Vec::new(),
            text,
            in_always_ff: false,
            in_always_comb: false,
            in_function: false,
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

    /// Semantic action for non-terminal 'IfResetStatement'
    fn if_reset_statement(&mut self, arg: &IfResetStatement) {
        if self.in_always_comb || self.in_function {
            self.errors.push(VerylError::invalid_statement(
                "if_reset",
                &self.text,
                &arg.if_reset.if_reset_token,
            ));
        }
        self.statement(&arg.statement);
        for x in &arg.if_reset_statement_list {
            self.expression(&x.expression);
            self.statement(&x.statement);
        }
        if let Some(ref x) = arg.if_reset_statement_opt {
            self.statement(&x.statement);
        }
    }

    /// Semantic action for non-terminal 'ReturnStatement'
    fn return_statement(&mut self, arg: &ReturnStatement) {
        if self.in_always_ff || self.in_always_comb {
            self.errors.push(VerylError::invalid_statement(
                "return",
                &self.text,
                &arg.r#return.return_token,
            ));
        }
        self.expression(&arg.expression);
    }

    /// Semantic action for non-terminal 'AlwaysFfDeclaration'
    fn always_ff_declaration(&mut self, arg: &AlwaysFfDeclaration) {
        // TODO check if_reset without first
        // Chcek first if_reset when reset signel exists
        let if_reset_required = if arg.always_ff_declaration_opt.is_some() {
            if let Some(ref x) = arg.always_ff_declaration_list.first() {
                match &*x.statement {
                    Statement::Statement2(_) => false,
                    _ => true,
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

        // Chcek reset signal when if_reset exists
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

        self.in_always_ff = true;
        self.always_ff_clock(&arg.always_ff_clock);
        if let Some(ref x) = arg.always_ff_declaration_opt {
            self.always_ff_reset(&x.always_ff_reset);
        }
        for x in &arg.always_ff_declaration_list {
            self.statement(&x.statement);
        }
        self.in_always_ff = false;
    }

    /// Semantic action for non-terminal 'AlwaysCombDeclaration'
    fn always_comb_declaration(&mut self, arg: &AlwaysCombDeclaration) {
        self.in_always_comb = true;
        for x in &arg.always_comb_declaration_list {
            self.statement(&x.statement);
        }
        self.in_always_comb = false;
    }

    /// Semantic action for non-terminal 'Direction'
    fn direction(&mut self, arg: &Direction) {
        match arg {
            Direction::Direction3(x) => {
                if !self.in_function {
                    self.errors.push(VerylError::invalid_direction(
                        "ref",
                        &self.text,
                        &x.r#ref.ref_token,
                    ));
                }
            }
            _ => (),
        };
    }

    /// Semantic action for non-terminal 'FunctionDeclaration'
    fn function_declaration(&mut self, arg: &FunctionDeclaration) {
        self.in_function = true;
        if let Some(ref x) = arg.function_declaration_opt {
            self.with_parameter(&x.with_parameter);
        }
        if let Some(ref x) = arg.function_declaration_opt0 {
            self.port_declaration(&x.port_declaration);
        }
        for x in &arg.function_declaration_list {
            self.function_item(&x.function_item);
        }
        self.in_function = false;
    }
}
