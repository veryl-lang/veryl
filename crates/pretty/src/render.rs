//! Wadler-style renderer for `Doc`. Each `Group` is laid out flat if it
//! fits within `max_width - col`; otherwise it breaks. Indent emission is
//! deferred so back-to-back newlines produce a clean blank line.

use crate::doc::{AnchoredText, CommentDoc, Doc};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub max_width: usize,
    pub indent_width: usize,
    pub newline: &'static str,
    /// Strip trailing whitespace from each output line. Disable when
    /// alignment padding must survive byte-for-byte.
    pub strip_trailing_whitespace: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            max_width: 120,
            indent_width: 4,
            newline: "\n",
            strip_trailing_whitespace: true,
        }
    }
}

struct State {
    out: String,
    col: usize,
    current_line: u32,
    /// If set, the next `Hardline` skips its `\n` because the previous
    /// emission already ended the line.
    swallow_next_break: bool,
    /// Spaces to emit before the next content. `None` while in the middle
    /// of a line. Deferred so back-to-back newlines yield true blank lines.
    pending_indent: Option<usize>,
    anchors: Vec<RenderedAnchor>,
}

/// 1-based source-map entry produced from a `Doc::Anchored`.
#[derive(Clone, Debug)]
pub struct RenderedAnchor {
    pub dst_line: u32,
    pub dst_column: u32,
    pub src_line: u32,
    pub src_column: u32,
    pub text: Rc<str>,
}

#[derive(Clone, Debug, Default)]
pub struct Rendered {
    pub text: String,
    pub anchors: Vec<RenderedAnchor>,
}

/// Render a document into a string, discarding anchors. Use
/// `render_with_anchors` to capture them.
pub fn render(doc: &Doc, opts: &RenderOpts) -> String {
    render_inner(doc, opts).text
}

pub fn render_with_anchors(doc: &Doc, opts: &RenderOpts) -> Rendered {
    render_inner(doc, opts)
}

fn render_inner(doc: &Doc, opts: &RenderOpts) -> Rendered {
    let mut state = State {
        out: String::new(),
        col: 0,
        current_line: 1,
        swallow_next_break: false,
        pending_indent: None,
        anchors: Vec::new(),
    };
    let mut stack: Vec<Frame<'_>> = Vec::with_capacity(64);
    stack.push(Frame {
        indent: 0,
        mode: Mode::Break,
        doc,
    });
    while let Some(frame) = stack.pop() {
        render_frame(frame, opts, &mut state, &mut stack);
    }
    let text = if opts.strip_trailing_whitespace {
        strip_trailing_whitespace(&state.out, opts.newline)
    } else {
        state.out
    };
    Rendered {
        text,
        anchors: state.anchors,
    }
}

struct Frame<'a> {
    indent: i32,
    mode: Mode,
    doc: &'a Doc,
}

fn flush_pending(state: &mut State) {
    if let Some(pad) = state.pending_indent.take() {
        for _ in 0..pad {
            state.out.push(' ');
        }
        state.col = pad;
    }
}

/// Like `flush_pending` but retargets downwards if the current frame's
/// indent is shallower than the queued value (the queue was set in a
/// nested frame but the content lives in an outer one). Never retargets
/// upwards.
fn flush_pending_with_indent(state: &mut State, frame_indent: i32, opts: &RenderOpts) {
    if let Some(pad) = state.pending_indent.take() {
        let target = pad_for(frame_indent, opts).min(pad);
        for _ in 0..target {
            state.out.push(' ');
        }
        state.col = target;
    }
}

fn pad_for(indent: i32, opts: &RenderOpts) -> usize {
    (indent.max(0) as usize) * opts.indent_width
}

fn render_frame<'a>(
    frame: Frame<'a>,
    opts: &RenderOpts,
    state: &mut State,
    stack: &mut Vec<Frame<'a>>,
) {
    let Frame { indent, mode, doc } = frame;
    match doc {
        Doc::Nil => {}
        Doc::Text(s) => {
            flush_pending_with_indent(state, indent, opts);
            // Real content invalidates a pending swallow.
            state.swallow_next_break = false;
            state.out.push_str(s);
            let nls = s.matches('\n').count() as u32;
            if nls == 0 {
                state.col += s.chars().count();
            } else {
                state.current_line += nls;
                let last_line = s.rsplit('\n').next().unwrap_or("");
                state.col = last_line.chars().count();
            }
        }
        Doc::Concat(items) => {
            for item in items.iter().rev() {
                stack.push(Frame {
                    indent,
                    mode,
                    doc: item,
                });
            }
        }
        Doc::Indent(off, inner) => {
            stack.push(Frame {
                indent: indent + off,
                mode,
                doc: inner,
            });
        }
        Doc::Group(inner) => {
            // Inherit Flat from a surrounding `ForceFlat`.
            let chosen = if matches!(mode, Mode::Flat) {
                Mode::Flat
            } else {
                let remaining = opts.max_width.saturating_sub(state.col);
                if fits_flat(inner, stack, remaining as isize) {
                    Mode::Flat
                } else {
                    Mode::Break
                }
            };
            stack.push(Frame {
                indent,
                mode: chosen,
                doc: inner,
            });
        }
        Doc::ForceFlat(inner) => {
            stack.push(Frame {
                indent,
                mode: Mode::Flat,
                doc: inner,
            });
        }
        Doc::Line(sep) => match mode {
            Mode::Flat => {
                flush_pending(state);
                state.swallow_next_break = false;
                state.out.push_str(sep);
                state.col += sep.chars().count();
            }
            Mode::Break => {
                emit_break(state, indent, opts);
            }
        },
        Doc::Hardline => match mode {
            Mode::Flat => {
                // Reachable only under `ForceFlat`: collapse to a space.
                flush_pending(state);
                state.swallow_next_break = false;
                state.out.push(' ');
                state.col += 1;
            }
            Mode::Break => {
                emit_break(state, indent, opts);
            }
        },
        Doc::DedentHardline(level) => match mode {
            Mode::Flat => {
                flush_pending(state);
                state.swallow_next_break = false;
                state.out.push(' ');
                state.col += 1;
            }
            Mode::Break => {
                // Strip up to `level * indent_width` trailing spaces so
                // alignment padding doesn't survive the dedent.
                let want = (*level as usize) * opts.indent_width;
                if want > 0 && state.pending_indent.is_none() {
                    let len = state.out.len();
                    let bytes = state.out.as_bytes();
                    if len >= want && bytes[len - want..].iter().all(|b| *b == b' ') {
                        state.out.truncate(len - want);
                        state.col = state.col.saturating_sub(want);
                    }
                }
                emit_break(state, indent, opts);
            }
        },
        Doc::Comments(cs) => {
            render_comments(cs, indent, opts, state);
        }
        Doc::IfBreak(s) => {
            if matches!(mode, Mode::Break) {
                flush_pending_with_indent(state, indent, opts);
                state.swallow_next_break = false;
                state.out.push_str(s);
                state.col += s.chars().count();
            }
        }
        Doc::IfBreakPad(width) => {
            if matches!(mode, Mode::Break) && *width > 0 {
                flush_pending_with_indent(state, indent, opts);
                state.swallow_next_break = false;
                for _ in 0..*width {
                    state.out.push(' ');
                }
                state.col += *width as usize;
            }
        }
        Doc::Pad(width) => {
            if *width > 0 {
                flush_pending_with_indent(state, indent, opts);
                state.swallow_next_break = false;
                for _ in 0..*width {
                    state.out.push(' ');
                }
                state.col += *width as usize;
            }
        }
        Doc::IfFlatPad(width) => {
            if matches!(mode, Mode::Flat) && *width > 0 {
                flush_pending_with_indent(state, indent, opts);
                state.swallow_next_break = false;
                for _ in 0..*width {
                    state.out.push(' ');
                }
                state.col += *width as usize;
            }
        }
        Doc::Anchored(a) => {
            emit_anchored(a, indent, opts, state);
        }
    }
}

fn emit_anchored(a: &Rc<AnchoredText>, indent: i32, opts: &RenderOpts, state: &mut State) {
    flush_pending_with_indent(state, indent, opts);
    state.swallow_next_break = false;
    state.anchors.push(RenderedAnchor {
        dst_line: state.current_line,
        dst_column: (state.col as u32) + 1,
        src_line: a.src_line,
        src_column: a.src_column,
        text: a.text.clone(),
    });
    state.out.push_str(&a.text);
    let nls = a.text.matches('\n').count() as u32;
    if nls == 0 {
        state.col += a.text.chars().count();
    } else {
        state.current_line += nls;
        let last = a.text.rsplit('\n').next().unwrap_or("");
        state.col = last.chars().count();
    }
}

fn emit_break(state: &mut State, indent: i32, opts: &RenderOpts) {
    if state.swallow_next_break {
        state.swallow_next_break = false;
    } else {
        state.out.push_str(opts.newline);
        state.current_line += 1;
        state.col = 0;
    }
    state.pending_indent = Some(pad_for(indent, opts));
}

fn render_comments(cs: &[CommentDoc], indent: i32, opts: &RenderOpts, state: &mut State) {
    let pad_width = pad_for(indent, opts);
    for c in cs {
        let pre_swallow = state.swallow_next_break;
        state.swallow_next_break = false;
        let pending = state.pending_indent.is_some();

        if c.leading_newlines == 0 && !pre_swallow && !pending {
            if state.col > 0 {
                state.out.push(' ');
                state.col += 1;
            }
        } else {
            // `pre_swallow` and `pending` collectively count as at most
            // one already-emitted `\n`; subtract that to avoid double-counting.
            let already = if pre_swallow || pending { 1u32 } else { 0 };
            let to_emit = c.leading_newlines.max(1).saturating_sub(already);
            for _ in 0..to_emit {
                state.out.push_str(opts.newline);
                state.current_line += 1;
                state.col = 0;
            }
            for _ in 0..pad_width {
                state.out.push(' ');
            }
            state.col = pad_width;
            state.pending_indent = None;
        }

        if c.src_line != 0 && c.src_column != 0 {
            state.anchors.push(RenderedAnchor {
                dst_line: state.current_line,
                dst_column: (state.col as u32) + 1,
                src_line: c.src_line,
                src_column: c.src_column,
                text: c.text.clone(),
            });
        }

        state.out.push_str(&c.text);
        let nls = c.text.matches('\n').count() as u32;
        if c.is_line_comment {
            state.out.push_str(opts.newline);
            state.current_line += nls + 1;
            state.col = 0;
            state.pending_indent = Some(pad_width);
            state.swallow_next_break = true;
        } else if nls > 0 {
            state.current_line += nls;
            state.col = 0;
        } else {
            state.col += c.text.chars().count();
        }
    }
}

/// Lay out the candidate flat, then continue into the outer continuation
/// until the first break opportunity ends the line. Without the
/// continuation, neighbouring siblings in a fill-mode list each declare
/// themselves "flat" individually and the composed line can overflow.
fn fits_flat(d: &Doc, outer: &[Frame<'_>], budget: isize) -> bool {
    let mut budget = budget;
    if budget < 0 {
        return false;
    }
    let mut work: Vec<(&Doc, bool)> = Vec::with_capacity(outer.len() + 8);
    for frame in outer.iter() {
        work.push((frame.doc, false));
    }
    work.push((d, true));
    while let Some((x, in_start)) = work.pop() {
        if budget < 0 {
            return false;
        }
        match x {
            Doc::Nil => {}
            Doc::Text(s) => {
                budget -= s.chars().count() as isize;
            }
            Doc::Concat(items) => {
                for item in items.iter().rev() {
                    work.push((item, in_start));
                }
            }
            Doc::Indent(_, inner) | Doc::Group(inner) | Doc::ForceFlat(inner) => {
                work.push((inner, in_start));
            }
            Doc::Line(sep) => {
                if in_start {
                    budget -= sep.chars().count() as isize;
                } else {
                    // A Line in the outer continuation is a break
                    // opportunity — the current line ends here.
                    return true;
                }
            }
            Doc::Hardline | Doc::DedentHardline(_) => {
                // In start: flat is impossible. In outer continuation:
                // this is the line ender, so the budget so far is what
                // matters.
                return !in_start;
            }
            Doc::IfBreak(_) | Doc::IfBreakPad(_) => {}
            Doc::Pad(width) | Doc::IfFlatPad(width) => {
                budget -= *width as isize;
            }
            Doc::Anchored(a) => {
                budget -= a.text.chars().count() as isize;
            }
            Doc::Comments(cs) => {
                // A line comment terminates the current line. Same
                // start/continuation rule as `Hardline`.
                if cs.iter().any(|c| c.is_line_comment) {
                    return !in_start;
                }
                for c in cs.iter() {
                    budget -= c.text.chars().count() as isize + 1;
                }
            }
        }
    }
    budget >= 0
}

/// Strip trailing whitespace from each line of `s`.
fn strip_trailing_whitespace(s: &str, newline: &'static str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split(newline).enumerate() {
        if i > 0 {
            out.push_str(newline);
        }
        let trimmed = line.trim_end_matches([' ', '\t']);
        out.push_str(trimmed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::*;

    fn opts(max: usize) -> RenderOpts {
        RenderOpts {
            max_width: max,
            indent_width: 4,
            newline: "\n",
            strip_trailing_whitespace: true,
        }
    }

    #[test]
    fn text_only() {
        let d = text("hello");
        assert_eq!(render(&d, &opts(80)), "hello");
    }

    #[test]
    fn group_fits_flat() {
        let d = group(concat(vec![
            text("a"),
            line(),
            text("b"),
            line(),
            text("c"),
        ]));
        assert_eq!(render(&d, &opts(80)), "a b c");
    }

    #[test]
    fn group_breaks_when_too_wide() {
        let d = group(nest(concat(vec![
            text("aaaa"),
            line(),
            text("bbbb"),
            line(),
            text("cccc"),
        ])));
        assert_eq!(render(&d, &opts(8)), "aaaa\n    bbbb\n    cccc");
    }

    #[test]
    fn hardline_forces_break() {
        let d = concat(vec![text("a"), hard(), text("b")]);
        assert_eq!(render(&d, &opts(80)), "a\nb");
    }

    #[test]
    fn nested_group_outer_breaks_inner_flat() {
        let inner = group(concat(vec![text("xx"), line(), text("yy")]));
        let outer = group(nest(concat(vec![
            text("[start]"),
            line(),
            inner.clone(),
            line(),
            text("[end]"),
        ])));
        assert_eq!(render(&outer, &opts(10)), "[start]\n    xx yy\n    [end]");
    }

    #[test]
    fn binary_op_break_when_overflow() {
        let parts = vec![
            text("aaa"),
            group(concat(vec![line(), text("+ bbb")])),
            group(concat(vec![line(), text("+ ccc")])),
            group(concat(vec![line(), text("+ ddd")])),
        ];
        let d = group(nest(concat(parts)));
        assert_eq!(render(&d, &opts(12)), "aaa + bbb\n    + ccc\n    + ddd");
    }

    #[test]
    fn outer_group_all_break() {
        let d = group(nest(concat(vec![
            text("aaa"),
            line(),
            text("+ bbb"),
            line(),
            text("+ ccc"),
            line(),
            text("+ ddd"),
        ])));
        assert_eq!(
            render(&d, &opts(12)),
            "aaa\n    + bbb\n    + ccc\n    + ddd"
        );
    }

    #[test]
    fn back_to_back_hardlines_produce_blank_line() {
        let d = concat(vec![text("a"), hard(), hard(), text("b")]);
        assert_eq!(render(&d, &opts(80)), "a\n\nb");
    }

    #[test]
    fn if_break_pad_emits_only_in_break_mode() {
        // Short content fits flat: padding is suppressed.
        let flat = group(concat(vec![
            text("a"),
            if_break_pad(3),
            text(":"),
            line(),
            text("u32"),
        ]));
        assert_eq!(render(&flat, &opts(80)), "a: u32");

        // Same doc forced to break by width: padding is emitted.
        let broken = group(nest(concat(vec![
            text("aaaaaaaa"),
            if_break_pad(3),
            text(":"),
            line(),
            text("u32"),
        ])));
        assert_eq!(render(&broken, &opts(6)), "aaaaaaaa   :\n    u32");
    }
}
