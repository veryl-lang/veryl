use crate::HashMap;
use std::fmt;
use veryl_parser::resource_table::PathId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::{Token, TokenSource};

#[derive(Clone, Debug)]
pub struct RangeTable<T> {
    table: HashMap<PathId, Vec<(TokenRange, T)>>,
    temporary: Vec<(Token, Option<T>)>,
}

impl<T> Default for RangeTable<T> {
    fn default() -> Self {
        Self {
            table: HashMap::default(),
            temporary: Vec::new(),
        }
    }
}

impl<T> RangeTable<T>
where
    T: Clone + Eq + std::fmt::Display,
{
    pub fn insert(&mut self, range: TokenRange, value: T) {
        if let TokenSource::File { path, .. } = range.beg.source {
            self.table
                .entry(path)
                .and_modify(|x| x.push((range, value.clone())))
                .or_insert(vec![(range, value)]);
        } else {
            unreachable!();
        }
    }

    pub fn begin(&mut self, token: Token, value: Option<T>) {
        self.temporary.push((token, value));
    }

    pub fn end(&mut self, token: Token) {
        let (beg, value) = self.temporary.pop().unwrap();
        if let Some(value) = value {
            let range = TokenRange { beg, end: token };
            self.insert(range, value);
        }
    }

    pub fn get(&self, token: &Token) -> Vec<T> {
        let mut ret = Vec::new();

        if let TokenSource::File { path, .. } = token.source
            && let Some(values) = self.table.get(&path)
        {
            for (range, value) in values {
                if range.include(path, token.line, token.column) {
                    ret.push(value.clone());
                }
            }
        }

        // Append values which are not closed
        for (_, t) in &self.temporary {
            if let Some(t) = t {
                ret.push(t.clone());
            }
        }

        ret
    }

    pub fn contains(&self, token: &Token, value: &T) -> bool {
        let attrs = self.get(token);
        attrs.contains(value)
    }

    pub fn dump(&self) -> String {
        format!("{self}")
    }

    pub fn get_all(&self) -> Vec<(TokenRange, T)> {
        self.table.values().flat_map(|x| x.clone()).collect()
    }

    pub fn clear(&mut self) {
        self.table.clear()
    }

    pub fn drop(&mut self, path: PathId) {
        self.table.retain(|x, _| *x != path)
    }
}

impl<T> fmt::Display for RangeTable<T>
where
    T: std::fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "[")?;
        let mut value_width = 0;
        let mut vec: Vec<_> = self.table.iter().collect();
        vec.sort_by(|x, y| format!("{}", x.0).cmp(&format!("{}", y.0)));
        for (_, v) in &vec {
            for (_, value) in v.iter() {
                value_width = value_width.max(format!("{value}").len());
            }
        }

        for (_, v) in &vec {
            for (range, value) in v.iter() {
                writeln!(
                    f,
                    "    {:value_width$} @ {}: {}:{} - {}:{},",
                    value,
                    range.beg.source,
                    range.beg.line,
                    range.beg.column,
                    range.end.line,
                    range.end.column,
                    value_width = value_width,
                )?;
            }
        }

        writeln!(f, "]")?;

        Ok(())
    }
}
