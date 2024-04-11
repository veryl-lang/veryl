use crate::attribute::Attribute;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::PathId;
use veryl_parser::veryl_token::{Token, TokenRange, TokenSource};

#[derive(Clone, Debug, Default)]
pub struct AttributeTable {
    table: HashMap<PathId, Vec<(TokenRange, Attribute)>>,
    temporary: Vec<(Token, Option<Attribute>)>,
}

impl AttributeTable {
    pub fn insert(&mut self, range: TokenRange, attr: Attribute) {
        if let TokenSource::File(path) = range.beg.source {
            self.table
                .entry(path)
                .and_modify(|x| x.push((range, attr)))
                .or_insert(vec![(range, attr)]);
        } else {
            unreachable!();
        }
    }

    pub fn begin(&mut self, token: Token, attr: Option<Attribute>) {
        self.temporary.push((token, attr));
    }

    pub fn end(&mut self, token: Token) {
        let (beg, attr) = self.temporary.pop().unwrap();
        if let Some(attr) = attr {
            let range = TokenRange { beg, end: token };
            self.insert(range, attr);
        }
    }

    pub fn get(&self, token: &Token) -> Vec<Attribute> {
        let mut ret = Vec::new();

        if let TokenSource::File(path) = token.source {
            if let Some(attrs) = self.table.get(&path) {
                for (range, attr) in attrs {
                    if range.include(path, token.line, token.column) {
                        ret.push(*attr);
                    }
                }
            }
        }

        // Append attributes which are not closed
        for (_, t) in &self.temporary {
            if let Some(t) = t {
                ret.push(*t);
            }
        }

        ret
    }

    pub fn contains(&self, token: &Token, attr: Attribute) -> bool {
        let attrs = self.get(token);
        attrs.contains(&attr)
    }

    pub fn dump(&self) -> String {
        format!("{self}")
    }

    pub fn get_all(&self) -> Vec<(TokenRange, Attribute)> {
        self.table.values().flat_map(|x| x.clone()).collect()
    }
}

impl fmt::Display for AttributeTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "AttributeTable [")?;
        let mut attr_width = 0;
        let mut vec: Vec<_> = self.table.iter().collect();
        vec.sort_by(|x, y| format!("{}", x.0).cmp(&format!("{}", y.0)));
        for (_, v) in &vec {
            for (_, attr) in v.iter() {
                attr_width = attr_width.max(format!("{attr}").len());
            }
        }

        for (_, v) in &vec {
            for (range, attr) in v.iter() {
                writeln!(
                    f,
                    "    {:attr_width$} @ {}: {}:{} - {}:{},",
                    attr,
                    range.beg.source,
                    range.beg.line,
                    range.beg.column,
                    range.end.line,
                    range.end.column,
                    attr_width = attr_width,
                )?;
            }
        }

        writeln!(f, "]")?;

        Ok(())
    }
}

thread_local!(static ATTRIBUTE_TABLE: RefCell<AttributeTable> = RefCell::new(AttributeTable::default()));

pub fn insert(range: TokenRange, attr: Attribute) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().insert(range, attr))
}

pub fn begin(token: Token, attr: Option<Attribute>) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().begin(token, attr))
}

pub fn end(token: Token) {
    ATTRIBUTE_TABLE.with(|f| f.borrow_mut().end(token))
}

pub fn get(token: &Token) -> Vec<Attribute> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get(token))
}

pub fn contains(token: &Token, attr: Attribute) -> bool {
    ATTRIBUTE_TABLE.with(|f| f.borrow().contains(token, attr))
}

pub fn dump() -> String {
    ATTRIBUTE_TABLE.with(|f| f.borrow().dump())
}

pub fn get_all() -> Vec<(TokenRange, Attribute)> {
    ATTRIBUTE_TABLE.with(|f| f.borrow().get_all())
}
