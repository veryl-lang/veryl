use crate::veryl_grammar_trait::*;
use crate::veryl_token::{Token, VerylToken};
use crate::veryl_walker::VerylWalker;
use veryl_metadata::{Format, Metadata};
use veryl_parser::resource_table;
use veryl_parser::veryl_grammar_trait::Veryl as NewVeryl;

pub struct Migrator {
    format_opt: Format,
    newline: &'static str,
    string: String,
    line: u32,
    column: u32,
}

impl Default for Migrator {
    fn default() -> Self {
        Self {
            format_opt: Format::default(),
            newline: "\n",
            string: String::new(),
            line: 1,
            column: 1,
        }
    }
}

impl Migrator {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            format_opt: metadata.format.clone(),
            ..Default::default()
        }
    }

    pub fn migrate(&mut self, input: &Veryl, raw_input: &str) {
        self.newline = self.format_opt.newline_style.newline_str(raw_input);
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
            self.str(self.newline);
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

    /// Check whether the valid syntax tree should be migrated
    pub fn migratable(_veryl: &NewVeryl) -> bool {
        false
    }
}

impl VerylWalker for Migrator {
    fn veryl_token(&mut self, arg: &VerylToken) {
        self.token(arg);
    }

    fn for_statement(&mut self, arg: &ForStatement) {
        self.r#for(&arg.r#for);
        self.identifier(&arg.identifier);
        self.r#in(&arg.r#in);
        if let Some(ref x) = arg.for_statement_opt {
            self.rev(&x.rev);
        }
        self.range(&arg.range);
        if let Some(ref x) = arg.for_statement_opt0 {
            self.step(&x.step);
            self.assignment_operator(&x.assignment_operator);
            self.expression(&x.expression);
        }
        self.statement_block(&arg.statement_block);
    }
}
