use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;
use veryl_metadata::Metadata;
use veryl_parser::resource_table;

#[cfg(target_os = "windows")]
const NEWLINE: &str = "\r\n";
#[cfg(not(target_os = "windows"))]
const NEWLINE: &str = "\n";

pub struct Migrator {
    string: String,
    line: u32,
    column: u32,
}

impl Default for Migrator {
    fn default() -> Self {
        Self {
            string: String::new(),
            line: 1,
            column: 1,
        }
    }
}

impl Migrator {
    pub fn new(_metadata: &Metadata) -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn migrate(&mut self, input: &Veryl) {
        self.veryl(input);
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    fn str(&mut self, x: &str) {
        self.string.push_str(x);
    }

    fn push_token(&mut self, x: &Token) {
        let newlines = x.line.saturating_sub(self.line);
        self.line = x.line;
        if newlines > 0 {
            self.column = 1;
        }
        let spaces = x.column.saturating_sub(self.column);
        self.column += spaces;

        for _ in 0..newlines {
            self.str(NEWLINE);
        }
        self.str(&" ".repeat(spaces as usize));

        let text = resource_table::get_str_value(x.text).unwrap();

        let newlines_in_text = text.matches('\n').count() as u32;
        self.line += newlines_in_text;
        let len = text.len() - text.rfind('\n').map(|x| x + 1).unwrap_or(0);
        if newlines_in_text > 0 {
            self.column = 1;
        } else {
            self.column += len as u32;
        }

        self.str(&text);
    }

    fn token(&mut self, x: &VerylToken) {
        self.push_token(&x.token);

        for x in &x.comments {
            self.push_token(x);
        }
    }
}

impl VerylWalker for Migrator {
    /// Semantic action for non-terminal 'VerylToken'
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    /// Semantic action for non-terminal 'Operator07'
    fn operator07(&mut self, arg: &Operator07) {
        let token = arg.operator07_token.token.text.to_string();
        match token.as_str() {
            "===" => {
                self.token(&arg.operator07_token.replace("=="));
            }
            "!==" => {
                self.token(&arg.operator07_token.replace("!="));
            }
            _ => {
                self.token(&arg.operator07_token);
            }
        }
    }

    /// Semantic action for non-terminal 'Operator05'
    fn operator05(&mut self, arg: &Operator05) {
        let token = arg.operator05_token.token.text.to_string();
        match token.as_str() {
            "^~" => {
                self.token(&arg.operator05_token.replace("~^"));
            }
            _ => {
                self.token(&arg.operator05_token);
            }
        }
    }
}
