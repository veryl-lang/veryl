//! Pure helpers that operate on `sv_parser::RefNode` plus the original SV
//! source text. Kept free-standing so the type/expression helpers in sibling
//! modules can use them without going through the `Converter` state.

use sv_parser::RefNode;

/// Return the substring of `src` spanning a node, computed from the offsets
/// of all `Locate` descendants. Returns `""` for nodes without locate info.
pub(crate) fn node_text<'a>(node: &RefNode, src: &'a str) -> &'a str {
    let mut start: Option<usize> = None;
    let mut end: usize = 0;
    for n in node.clone().into_iter() {
        if let RefNode::Locate(loc) = n {
            let s = loc.offset;
            let e = loc.offset + loc.len;
            if start.is_none() {
                start = Some(s);
            }
            if e > end {
                end = e;
            }
        }
    }
    if let Some(s) = start
        && end <= src.len()
        && s <= end
    {
        &src[s..end]
    } else {
        ""
    }
}

/// Return the 1-based source line number of the first `Locate` descendant.
pub(crate) fn node_line(node: &RefNode) -> usize {
    for n in node.clone().into_iter() {
        if let RefNode::Locate(loc) = n {
            return loc.line as usize;
        }
    }
    0
}
