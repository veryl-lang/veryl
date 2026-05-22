//! Doc IR for the Wadler-style pretty printer. Layout decisions are
//! deferred to `render.rs`.

use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct CommentDoc {
    pub text: Rc<str>,
    /// Newlines separating this comment from the previous emission.
    pub leading_newlines: u32,
    pub is_line_comment: bool,
    /// 1-based source position; 0 means "no anchor".
    pub src_line: u32,
    pub src_column: u32,
}

#[derive(Clone, Debug, Default)]
pub enum Doc {
    #[default]
    Nil,

    /// Plain text without `\n`.
    Text(Rc<str>),

    Concat(Rc<[Doc]>),

    /// Indent inner by `level * indent_width` at render time.
    Indent(i32, Rc<Doc>),

    /// Render flat if it fits within `max_width`, otherwise break.
    Group(Rc<Doc>),

    /// Force flat rendering; `Hardline` / `DedentHardline` inside collapse
    /// to a space.
    ForceFlat(Rc<Doc>),

    /// Soft line: emits `sep` when flat, newline + indent when broken.
    Line(&'static str),

    Hardline,

    /// Hardline that strips up to `level * indent_width` trailing spaces.
    DedentHardline(u32),

    Comments(Rc<[CommentDoc]>),

    /// Emit `text` only when the immediately enclosing group breaks;
    /// contributes 0 to `fits_flat`.
    IfBreak(Rc<str>),

    /// `IfBreak` for `width` spaces.
    IfBreakPad(u32),

    /// Emit `width` spaces unconditionally; counted as `width` in `fits_flat`.
    Pad(u32),

    /// Emit `width` spaces only when the immediately enclosing group is
    /// flat; counted as `width` in `fits_flat`.
    IfFlatPad(u32),

    /// `Text` plus a source-position anchor for source-map emission.
    Anchored(Rc<AnchoredText>),
}

#[derive(Clone, Debug)]
pub struct AnchoredText {
    pub text: Rc<str>,
    pub src_line: u32,
    pub src_column: u32,
}

pub fn text(s: impl Into<Rc<str>>) -> Doc {
    Doc::Text(s.into())
}

pub fn space(n: usize) -> Doc {
    if n == 0 {
        Doc::Nil
    } else {
        Doc::Text(" ".repeat(n).into())
    }
}

/// Space when flat, newline when broken.
pub fn line() -> Doc {
    Doc::Line(" ")
}

/// Empty when flat, newline when broken.
pub fn softline() -> Doc {
    Doc::Line("")
}

pub fn hard() -> Doc {
    Doc::Hardline
}

pub fn nest(d: Doc) -> Doc {
    Doc::Indent(1, Rc::new(d))
}

pub fn dedent(d: Doc) -> Doc {
    Doc::Indent(-1, Rc::new(d))
}

pub fn indent_by(level: i32, d: Doc) -> Doc {
    if level == 0 {
        d
    } else {
        Doc::Indent(level, Rc::new(d))
    }
}

pub fn group(d: Doc) -> Doc {
    Doc::Group(Rc::new(d))
}

pub fn force_flat(d: Doc) -> Doc {
    Doc::ForceFlat(Rc::new(d))
}

pub fn concat(docs: Vec<Doc>) -> Doc {
    let docs: Vec<Doc> = docs
        .into_iter()
        .filter(|d| !matches!(d, Doc::Nil))
        .collect();
    match docs.len() {
        0 => Doc::Nil,
        1 => docs.into_iter().next().unwrap(),
        _ => Doc::Concat(docs.into()),
    }
}

pub fn comments(cs: Vec<CommentDoc>) -> Doc {
    if cs.is_empty() {
        Doc::Nil
    } else {
        Doc::Comments(cs.into())
    }
}

pub fn if_break(text: impl Into<Rc<str>>) -> Doc {
    Doc::IfBreak(text.into())
}

pub fn if_break_pad(width: u32) -> Doc {
    if width == 0 {
        Doc::Nil
    } else {
        Doc::IfBreakPad(width)
    }
}

pub fn pad(width: u32) -> Doc {
    if width == 0 {
        Doc::Nil
    } else {
        Doc::Pad(width)
    }
}

pub fn if_flat_pad(width: u32) -> Doc {
    if width == 0 {
        Doc::Nil
    } else {
        Doc::IfFlatPad(width)
    }
}

pub fn anchored(text: impl Into<Rc<str>>, src_line: u32, src_column: u32) -> Doc {
    Doc::Anchored(Rc::new(AnchoredText {
        text: text.into(),
        src_line,
        src_column,
    }))
}
