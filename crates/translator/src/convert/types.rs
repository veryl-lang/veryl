//! SystemVerilog → Veryl type-string mapping. These helpers walk a sv-parser
//! `DataType` (or compatible) subtree and produce a Veryl type expression.

use super::util::node_text;
use sv_parser::RefNode;

/// Convert a SystemVerilog DataType / DataTypeOrImplicit subtree into a Veryl
/// type string. Walks the subtree to find IntegerAtomType, IntegerVectorType,
/// Signing and PackedDimensions and assembles them.
///
/// Returns an empty string if no recognised type information is present
/// (callers typically substitute a `logic` or `u32` default in that case).
pub(crate) fn sv_type_to_veryl(node: &RefNode, src: &str) -> String {
    let mut signed = false;
    let mut atom: Option<&str> = None;
    let mut vector_kind: Option<&str> = None;
    let mut packed: Vec<String> = Vec::new();
    let mut type_ident: Option<String> = None;

    for n in node.clone().into_iter() {
        match n {
            RefNode::Signing(_) => {
                if node_text(&n, src).trim() == "signed" {
                    signed = true;
                }
            }
            RefNode::IntegerAtomType(_) => {
                let txt = node_text(&n, src).trim();
                atom = Some(match txt {
                    "byte" => "i8",
                    "shortint" => "i16",
                    "int" => "i32",
                    "longint" => "i64",
                    "integer" => "i32",
                    "time" => "u64",
                    _ => "i32",
                });
            }
            RefNode::IntegerVectorType(_) => {
                let txt = node_text(&n, src).trim();
                vector_kind = Some(match txt {
                    "bit" => "bit",
                    _ => "logic",
                });
            }
            RefNode::PackedDimension(_) => {
                packed.push(node_text(&n, src).trim().to_string());
            }
            RefNode::TypeIdentifier(_) => {
                if type_ident.is_none() {
                    type_ident = Some(node_text(&n, src).trim().to_string());
                }
            }
            _ => {}
        }
    }

    if let Some(a) = atom {
        return a.to_string();
    }
    if let Some(t) = type_ident {
        return t;
    }
    let kind = vector_kind.unwrap_or("logic");
    let mut s = String::new();
    if signed {
        s.push_str("signed ");
    }
    s.push_str(kind);
    let widths: Vec<String> = packed
        .iter()
        .filter_map(|d| packed_dim_to_width(d))
        .collect();
    if !widths.is_empty() {
        s.push('<');
        s.push_str(&widths.join(", "));
        s.push('>');
    }
    s
}

/// Convert a SystemVerilog packed-dimension token (e.g. `[N-1:0]` or `[7:0]`)
/// into a Veryl width spec (`N`, `8`, `N+1`). Returns `None` for shapes the
/// helper doesn't understand (e.g. non-zero low bound).
pub(crate) fn packed_dim_to_width(dim: &str) -> Option<String> {
    let inner = dim.strip_prefix('[')?.strip_suffix(']')?;
    let (hi, lo) = inner.split_once(':')?;
    let hi = hi.trim();
    let lo = lo.trim();
    if lo != "0" {
        return None;
    }
    if let Ok(hi_n) = hi.parse::<i64>() {
        return Some((hi_n + 1).to_string());
    }
    if let Some(n) = hi.strip_suffix("-1") {
        Some(n.trim().to_string())
    } else {
        Some(format!("{hi}+1"))
    }
}

#[cfg(test)]
mod tests {
    use super::packed_dim_to_width;

    #[test]
    fn numeric_high_minus_one() {
        // [N-1:0] with numeric N folds to N.
        assert_eq!(packed_dim_to_width("[8-1:0]"), Some("8".to_string()));
    }

    #[test]
    fn numeric_high_inclusive() {
        // [7:0] folds to 8.
        assert_eq!(packed_dim_to_width("[7:0]"), Some("8".to_string()));
        assert_eq!(packed_dim_to_width("[15:0]"), Some("16".to_string()));
    }

    #[test]
    fn parametric_minus_one() {
        // [N-1:0] with non-numeric N keeps N as the width.
        assert_eq!(packed_dim_to_width("[N-1:0]"), Some("N".to_string()));
        assert_eq!(
            packed_dim_to_width("[WIDTH-1:0]"),
            Some("WIDTH".to_string())
        );
    }

    #[test]
    fn parametric_inclusive() {
        // [N:0] becomes N+1 (no folding possible).
        assert_eq!(packed_dim_to_width("[N:0]"), Some("N+1".to_string()));
    }

    #[test]
    fn nonzero_low_bound_unsupported() {
        assert_eq!(packed_dim_to_width("[7:1]"), None);
        assert_eq!(packed_dim_to_width("[N:1]"), None);
    }

    #[test]
    fn malformed_input_returns_none() {
        assert_eq!(packed_dim_to_width("[7]"), None);
        assert_eq!(packed_dim_to_width("7:0"), None);
        assert_eq!(packed_dim_to_width(""), None);
    }
}
