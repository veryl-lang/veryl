use std::collections::HashMap;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Location {
    pub line: u32,
    pub column: u32,
    pub length: u32,
    pub source: TokenSource,
    pub duplicated: Option<usize>,
}

impl From<&Token> for Location {
    fn from(x: &Token) -> Self {
        Self {
            line: x.line,
            column: x.column,
            length: x.length,
            source: x.source,
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
            source: x.source,
            duplicated: None,
        }
    }
}

/// When the renderer should emit an alignment-padding entry. The
/// aligner computes the padding *width*; this enum picks *when* it
/// becomes visible (and whether it counts toward `fits_flat`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PadKind {
    /// Always emit; contributes `width` to `fits_flat`.
    #[default]
    Always,
    /// Emit only when the enclosing layout group breaks; 0 in `fits_flat`.
    IfBreak,
    /// Emit only when the enclosing layout group is flat; contributes
    /// `width` so an overflow can force a break.
    IfFlat,
}

impl PadKind {
    /// `Always` wins so the padding can never be silently dropped under
    /// some layout decision.
    pub fn merge(self, other: PadKind) -> PadKind {
        match (self, other) {
            (PadKind::Always, _) | (_, PadKind::Always) => PadKind::Always,
            (PadKind::IfBreak, PadKind::IfBreak) => PadKind::IfBreak,
            (PadKind::IfFlat, PadKind::IfFlat) => PadKind::IfFlat,
            (PadKind::IfBreak, PadKind::IfFlat) | (PadKind::IfFlat, PadKind::IfBreak) => {
                PadKind::Always
            }
        }
    }
}

#[derive(Default)]
pub struct Align {
    enable: bool,
    /// Pad kind for the currently-open item; reset to `Always` on finish.
    pad_kind: PadKind,
    index: usize,
    max_width: u32,
    width: u32,
    line: u32,
    /// Whether this kind participated in the current statement. Lets
    /// `note_statement_end` advance `self.line` only for kinds that
    /// actually emitted, so intermediate non-participating statements
    /// still produce a natural group split.
    had_item_in_statement: bool,
    rest: Vec<(Location, u32, PadKind)>,
    additions: HashMap<Location, (u32, PadKind)>,
    disable_auto_finish: bool,
    pub last_location: Option<Location>,
}

impl Align {
    fn finish_group(&mut self) {
        for (loc, width, kind) in &self.rest {
            self.additions.insert(*loc, (self.max_width - width, *kind));
        }
        self.rest.clear();
        self.max_width = 0;
    }

    /// Clear the per-kind statement-participation flag. Use after an
    /// explicit `finish_group` that abandons the current statement
    /// context without going through `note_statement_end`.
    pub fn clear_had_item_in_statement(&mut self) {
        self.had_item_in_statement = false;
    }

    pub fn finish_item(&mut self) {
        if self.enable {
            self.enable = false;
            let kind = self.pad_kind;
            self.pad_kind = PadKind::default();
            if let Some(loc) = self.last_location {
                if !self.disable_auto_finish && (self.line > loc.line || loc.line - self.line > 1) {
                    self.finish_group();
                }
                self.max_width = u32::max(self.max_width, self.width);
                self.line = loc.line;
                self.rest.push((loc, self.width, kind));

                self.width = 0;
                self.index += 1;
            }
        }
    }

    pub fn start_item(&mut self) {
        if !self.enable {
            self.enable = true;
            self.width = 0;
            self.pad_kind = PadKind::Always;
            self.had_item_in_statement = true;
        }
    }

    /// `start_item` with `PadKind::IfBreak` — padding visible only in
    /// the broken layout.
    pub fn start_item_break_gated(&mut self) {
        if !self.enable {
            self.enable = true;
            self.width = 0;
            self.pad_kind = PadKind::IfBreak;
            self.had_item_in_statement = true;
        }
    }

    /// `start_item` with `PadKind::IfFlat` — padding visible only in
    /// the flat layout, and counted in `fits_flat`.
    pub fn start_item_flat_gated(&mut self) {
        if !self.enable {
            self.enable = true;
            self.width = 0;
            self.pad_kind = PadKind::IfFlat;
            self.had_item_in_statement = true;
        }
    }

    /// Carry `self.line` forward to the statement's end line, but
    /// only if this kind participated in the statement.
    pub fn note_statement_end(&mut self, line: u32) {
        if self.had_item_in_statement {
            if line > self.line {
                self.line = line;
            }
            self.had_item_in_statement = false;
        }
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
            self.last_location = Some(x);
        }
    }

    pub fn dummy_token(&mut self, x: &VerylToken) {
        if self.enable {
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

    pub fn add_width(&mut self, width: u32) {
        if self.enable {
            self.width += width;
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
    pub const NUMBER: usize = 9;
    pub const VAR_KEYWORD: usize = 10;
    /// Identifier column inside `::<...>` generic parameter / argument
    /// lists. Kept distinct from the outer IDENTIFIER kind so a
    /// nested generic doesn't merge into the surrounding column group.
    pub const GENERIC_IDENTIFIER: usize = 11;
    /// Type column inside `::<...>` generic parameter lists.
    pub const GENERIC_TYPE: usize = 12;
    /// Expression column inside `::<...>` generic parameter / argument
    /// lists.
    pub const GENERIC_EXPRESSION: usize = 13;
    /// Identifier column inside an inst's `#(...)` parameter list or
    /// `(...)` port list — isolated from the outer cross-inst column.
    pub const INST_ITEM_IDENTIFIER: usize = 14;
    /// Expression / value column inside an inst's `#(...)` or `(...)`.
    pub const INST_ITEM_EXPRESSION: usize = 15;
    /// The inst name's column. Distinct from `IDENTIFIER` because
    /// `inst` declarations interleave in the same scope as var/let/const
    /// but carry a wider leading keyword; sharing the kind would push
    /// shorter siblings' identifier columns out unexpectedly.
    pub const INST_NAME_IDENTIFIER: usize = 16;
    /// LHS column for `assign` / `connect` statements. Isolated from
    /// `IDENTIFIER` for the same reason as `INST_NAME_IDENTIFIER`.
    pub const ASSIGN_DECL_IDENTIFIER: usize = 17;
    /// Total number of alignment kinds. Used to size `Aligner::aligns`;
    /// keep in sync when adding a new kind.
    pub const COUNT: usize = 18;
}

#[derive(Default)]
pub struct Aligner {
    /// Per-token column padding to insert after the token; see
    /// `PadKind` for emit semantics.
    pub additions: HashMap<Location, (u32, PadKind)>,
    pub aligns: [Align; align_kind::COUNT],
    /// Latest source line observed by `token` / `duplicated_token`,
    /// consumed by `note_statement_end`.
    latest_observed_line: u32,
}

impl Aligner {
    pub fn new() -> Self {
        Default::default()
    }

    fn observe_line(&mut self, line: u32) {
        if line > self.latest_observed_line {
            self.latest_observed_line = line;
        }
    }

    pub fn token(&mut self, x: &VerylToken) {
        self.observe_line(x.token.line);
        for i in 0..self.aligns.len() {
            self.aligns[i].token(x);
        }
    }

    pub fn duplicated_token(&mut self, x: &VerylToken, idx: usize) {
        self.observe_line(x.token.line);
        for i in 0..self.aligns.len() {
            self.aligns[i].duplicated_token(x, idx);
        }
    }

    /// Signal the end of a statement. Carries every participating
    /// kind's reference line forward to `latest_observed_line`.
    pub fn note_statement_end(&mut self) {
        let line = self.latest_observed_line;
        for align in &mut self.aligns {
            align.note_statement_end(line);
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

    pub fn finish_item(&mut self) {
        for i in 0..self.aligns.len() {
            self.aligns[i].finish_item();
        }
    }

    /// Clear every kind's statement-participation flag. The auto-finish
    /// path inside `finish_item` deliberately leaves the flag set —
    /// only explicit alignment-context discards should call this.
    pub fn clear_had_item_in_statement(&mut self) {
        for align in &mut self.aligns {
            align.clear_had_item_in_statement();
        }
    }

    pub fn gather_additions(&mut self) {
        for align in &self.aligns {
            for (loc, (width, kind)) in &align.additions {
                self.additions
                    .entry(*loc)
                    .and_modify(|(val, kd)| {
                        *val += *width;
                        *kd = kd.merge(*kind);
                    })
                    .or_insert((*width, *kind));
            }
        }
    }

    pub fn enable_auto_finish(&mut self) {
        for align in &mut self.aligns {
            align.disable_auto_finish = false;
        }
    }

    pub fn disable_auto_finish(&mut self) {
        for align in &mut self.aligns {
            align.disable_auto_finish = true;
        }
    }

    pub fn enable_auto_finish_for(&mut self, kind: usize) {
        self.aligns[kind].disable_auto_finish = false;
    }

    /// Suppress the source-line-gap-based auto split for one kind.
    /// Required when grouping must stay structural (idempotent) — see
    /// `case_expression`.
    pub fn disable_auto_finish_for(&mut self, kind: usize) {
        self.aligns[kind].disable_auto_finish = true;
    }

    pub fn any_enabled(&self) -> bool {
        self.aligns.iter().any(|x| x.enable)
    }
}
