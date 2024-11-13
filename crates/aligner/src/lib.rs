use std::collections::HashMap;
use veryl_parser::veryl_token::{Token, VerylToken};

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: u32,
    pub column: u32,
    pub length: u32,
    pub duplicated: Option<usize>,
}

impl From<&Token> for Location {
    fn from(x: &Token) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

impl From<Token> for Location {
    fn from(x: Token) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            duplicated: None,
        }
    }
}

#[derive(Default)]
pub struct Align {
    enable: bool,
    index: usize,
    max_width: u32,
    width: u32,
    line: u32,
    rest: Vec<(Location, u32)>,
    additions: HashMap<Location, u32>,
    pub last_location: Option<Location>,
}

impl Align {
    fn finish_group(&mut self) {
        for (loc, width) in &self.rest {
            self.additions.insert(*loc, self.max_width - width);
        }
        self.rest.clear();
        self.max_width = 0;
    }

    pub fn finish_item(&mut self) {
        self.enable = false;
        if let Some(loc) = self.last_location {
            if self.line > loc.line || loc.line - self.line > 1 {
                self.finish_group();
            }
            self.max_width = u32::max(self.max_width, self.width);
            self.line = loc.line;
            self.rest.push((loc, self.width));

            self.width = 0;
            self.index += 1;
        }
    }

    pub fn start_item(&mut self) {
        self.enable = true;
        self.width = 0;
    }

    fn token(&mut self, x: &VerylToken) {
        if self.enable {
            self.width += x.token.length;
            let loc: Location = x.token.into();
            self.last_location = Some(loc);
        }
    }

    pub fn dummy_location(&mut self, x: Location) {
        if self.enable {
            self.width += 0; // 0 length token
            self.last_location = Some(x);
        }
    }

    pub fn dummy_token(&mut self, x: &VerylToken) {
        if self.enable {
            self.width += 0; // 0 length token
            let loc: Location = x.token.into();
            self.last_location = Some(loc);
        }
    }

    pub fn duplicated_token(&mut self, x: &VerylToken, i: usize) {
        if self.enable {
            self.width += x.token.length;
            let mut loc: Location = x.token.into();
            loc.duplicated = Some(i);
            self.last_location = Some(loc);
        }
    }

    fn space(&mut self, x: usize) {
        if self.enable {
            self.width += x as u32;
        }
    }
}

pub mod align_kind {
    pub const IDENTIFIER: usize = 0;
    pub const TYPE: usize = 1;
    pub const EXPRESSION: usize = 2;
    pub const WIDTH: usize = 3;
    pub const ARRAY: usize = 4;
    pub const ASSIGNMENT: usize = 5;
    pub const PARAMETER: usize = 6;
    pub const DIRECTION: usize = 7;
    pub const CLOCK_DOMAIN: usize = 8;
}

#[derive(Default)]
pub struct Aligner {
    pub additions: HashMap<Location, u32>,
    pub aligns: [Align; 9],
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn token(&mut self, x: &VerylToken) {
        for i in 0..self.aligns.len() {
            self.aligns[i].token(x);
        }
    }

    pub fn space(&mut self, x: usize) {
        for i in 0..self.aligns.len() {
            self.aligns[i].space(x);
        }
    }

    pub fn finish_group(&mut self) {
        for i in 0..self.aligns.len() {
            self.aligns[i].finish_group();
        }
    }

    pub fn gather_additions(&mut self) {
        for align in &self.aligns {
            for (x, y) in &align.additions {
                self.additions
                    .entry(*x)
                    .and_modify(|val| *val += *y)
                    .or_insert(*y);
            }
        }
    }
}
