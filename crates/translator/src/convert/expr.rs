//! Best-effort textual rewrites that bridge SystemVerilog expression syntax to
//! Veryl. The translator copies most expression text verbatim from the SV
//! source; the helpers here are applied just before emitting RHS / call /
//! statement text to repair the cases where the syntaxes diverge.

/// Rewrite SystemVerilog cast expressions into Veryl `as` form.
///
/// Handles `N'(x)` → `(x) as logic<N>` and `T'(x)` → `(x) as T`. Other text
/// passes through unchanged. Designed to be cheap and conservative — if the
/// pattern can't be parsed unambiguously, the original text is preserved.
pub(crate) fn expr_text_to_veryl(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            // Find the prefix immediately before the `'(`. The prefix is the
            // trailing token in `out`, optionally surrounded by parentheses.
            let mut prefix_end = out.len();
            while prefix_end > 0 && out.as_bytes()[prefix_end - 1].is_ascii_whitespace() {
                prefix_end -= 1;
            }
            let mut prefix_start = prefix_end;
            if prefix_start > 0 && out.as_bytes()[prefix_start - 1] == b')' {
                let mut depth = 1;
                prefix_start -= 1;
                while prefix_start > 0 {
                    prefix_start -= 1;
                    match out.as_bytes()[prefix_start] {
                        b')' => depth += 1,
                        b'(' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                while prefix_start > 0 {
                    let c = out.as_bytes()[prefix_start - 1];
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        prefix_start -= 1;
                    } else {
                        break;
                    }
                }
            }
            if prefix_start == prefix_end {
                out.push('\'');
                i += 1;
                continue;
            }
            // Find the matching close paren for `'(`.
            let mut j = i + 2;
            let mut depth = 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth != 0 {
                out.push('\'');
                i += 1;
                continue;
            }
            let prefix = out[prefix_start..prefix_end].to_string();
            let inner = &s[i + 2..j - 1];
            let trimmed = prefix.trim_matches(|c| c == '(' || c == ')').trim();
            let target = if trimmed.chars().all(|c| c.is_ascii_digit()) {
                format!("logic<{trimmed}>")
            } else {
                trimmed.to_string()
            };
            out.truncate(prefix_start);
            out.push_str(&format!("({}) as {}", inner, target));
            i = j;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::expr_text_to_veryl;

    #[test]
    fn passthrough_when_no_cast() {
        assert_eq!(expr_text_to_veryl("a + b"), "a + b");
        assert_eq!(expr_text_to_veryl("foo(x, y)"), "foo(x, y)");
        assert_eq!(expr_text_to_veryl(""), "");
    }

    #[test]
    fn numeric_width_cast() {
        assert_eq!(expr_text_to_veryl("8'(x)"), "(x) as logic<8>");
        assert_eq!(expr_text_to_veryl("16'(a + b)"), "(a + b) as logic<16>");
    }

    #[test]
    fn type_cast() {
        assert_eq!(expr_text_to_veryl("byte_t'(x)"), "(x) as byte_t");
        assert_eq!(expr_text_to_veryl("state_e'(s)"), "(s) as state_e");
    }

    #[test]
    fn parenthesised_width_cast() {
        assert_eq!(expr_text_to_veryl("(8)'(x)"), "(x) as logic<8>");
    }

    #[test]
    fn cast_inside_expression() {
        assert_eq!(expr_text_to_veryl("y + 8'(z)"), "y + (z) as logic<8>");
    }

    #[test]
    fn unrecognised_apostrophe_passes_through() {
        // SV literal `'0` and friends — no `(` follows, must not be touched.
        assert_eq!(expr_text_to_veryl("'0"), "'0");
        assert_eq!(expr_text_to_veryl("8'h0a"), "8'h0a");
    }
}
