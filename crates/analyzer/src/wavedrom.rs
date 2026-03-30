use crate::AnalyzerError;
use crate::analyzer_error::InvalidWavedromKind;
use crate::symbol::WavedromBlock;
use veryl_parser::resource_table::{self, PathId, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::Token;

/// Convert JSON5-style WaveDrom string to standard JSON.
pub fn preprocess_json5(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        match chars[i] {
            '\'' => {
                result.push('"');
                i += 1;
                while i < len && chars[i] != '\'' {
                    if chars[i] == '"' {
                        result.push('\\');
                        result.push('"');
                    } else if chars[i] == '\\' && i + 1 < len {
                        result.push(chars[i]);
                        i += 1;
                        result.push(chars[i]);
                    } else {
                        result.push(chars[i]);
                    }
                    i += 1;
                }
                result.push('"');
                if i < len {
                    i += 1;
                }
            }
            '"' => {
                result.push('"');
                i += 1;
                while i < len && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < len {
                        result.push(chars[i]);
                        i += 1;
                        result.push(chars[i]);
                    } else {
                        result.push(chars[i]);
                    }
                    i += 1;
                }
                if i < len {
                    result.push('"');
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < len
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let mut j = i;
                while j < len && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < len && chars[j] == ':' {
                    result.push('"');
                    result.push_str(&word);
                    result.push('"');
                } else {
                    result.push_str(&word);
                }
            }
            '/' if i + 1 < len && chars[i + 1] == '/' => {
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < len && chars[i + 1] == '*' => {
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                }
            }
            c => {
                result.push(c);
                i += 1;
            }
        }
    }

    // Remove trailing commas: `,` followed by optional whitespace then `]` or `}`
    let mut cleaned = String::with_capacity(result.len());
    let result_chars: Vec<char> = result.chars().collect();
    let rlen = result_chars.len();
    let mut ri = 0;
    while ri < rlen {
        if result_chars[ri] == ',' {
            let mut j = ri + 1;
            while j < rlen && result_chars[j].is_whitespace() {
                j += 1;
            }
            if j < rlen && (result_chars[j] == ']' || result_chars[j] == '}') {
                ri += 1;
                continue;
            }
        }
        cleaned.push(result_chars[ri]);
        ri += 1;
    }

    cleaned
}

/// Strip common port prefixes (i_, o_, io_).
pub fn strip_port_prefix(name: &str) -> &str {
    if let Some(stripped) = name.strip_prefix("i_") {
        stripped
    } else if let Some(stripped) = name.strip_prefix("o_") {
        stripped
    } else if let Some(stripped) = name.strip_prefix("io_") {
        stripped
    } else {
        name
    }
}

pub struct DocTestTarget {
    pub module_name: StrId,
    pub wavedrom_json: String,
    pub ports: Vec<(String, String)>,
    pub path: PathId,
}

/// Build a TokenRange pointing to a specific source line by computing its byte offset.
fn doc_line_token(module_token: &Token, line: u32) -> Option<TokenRange> {
    let source_text = module_token.source.get_text();
    let mut pos = 0u32;
    // Use split('\n') instead of lines() to preserve \r for correct byte offsets
    for (i, src_line) in source_text.split('\n').enumerate() {
        // 1-based line numbers
        if i as u32 + 1 == line {
            let trimmed = src_line.trim_end_matches('\r');
            let length = trimmed.len() as u32;
            let text = resource_table::insert_str(trimmed.trim());
            let id = resource_table::new_token_id();
            let token = Token {
                id,
                text,
                line,
                column: 1,
                length,
                pos,
                source: module_token.source,
            };
            return Some(token.into());
        }
        pos += src_line.len() as u32 + 1; // +1 for \n
    }
    None
}

fn signal_hint(kind: &InvalidWavedromKind) -> Option<&str> {
    match kind {
        InvalidWavedromKind::UnknownWaveChar { signal, .. } => Some(signal),
        InvalidWavedromKind::PipeSeparatorInTest { signal } => Some(signal),
        _ => None,
    }
}

/// Resolve error location: the specific signal line if available, otherwise the whole block.
fn resolve_error_line(
    module_token: &Token,
    block: &WavedromBlock,
    kind: &InvalidWavedromKind,
    fallback: TokenRange,
) -> TokenRange {
    if let Some(hint) = signal_hint(kind)
        && let Some(line) = block.find_line_containing(hint)
        && let Some(token) = doc_line_token(module_token, line)
    {
        return token;
    }

    if let Some(beg) = doc_line_token(module_token, block.fence_line)
        && let Some(end) = doc_line_token(module_token, block.end_line)
    {
        return TokenRange {
            beg: beg.beg,
            end: end.end,
        };
    }

    fallback
}

fn parse_wavedrom_json(json_str: &str) -> Result<serde_json::Value, InvalidWavedromKind> {
    let json_str = preprocess_json5(json_str);
    serde_json::from_str(&json_str).map_err(|e| InvalidWavedromKind::InvalidJson(e.to_string()))
}

fn check_wave_chars(signal_array: &[serde_json::Value]) -> Result<(), InvalidWavedromKind> {
    for item in signal_array {
        if item.is_string() || item.is_array() || item.as_object().is_none_or(|o| o.is_empty()) {
            continue;
        }
        if let Some(obj) = item.as_object()
            && let Some(name) = obj.get("name").and_then(|v| v.as_str())
            && let Some(wave) = obj.get("wave").and_then(|v| v.as_str())
        {
            for c in wave.chars() {
                if !matches!(
                    c,
                    'p' | 'P'
                        | 'n'
                        | 'N'
                        | '0'
                        | '1'
                        | 'x'
                        | 'z'
                        | '.'
                        | '|'
                        | '2'
                        | '3'
                        | '4'
                        | '5'
                        | '6'
                        | '7'
                        | '8'
                        | '9'
                        | '='
                        | ' '
                ) {
                    return Err(InvalidWavedromKind::UnknownWaveChar {
                        signal: name.to_string(),
                        ch: c,
                    });
                }
            }
        }
    }
    Ok(())
}

fn is_javascript_expression(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.starts_with('{') && !trimmed.starts_with('[')
}

fn validate_wavedrom_syntax(json_str: &str) -> Result<(), InvalidWavedromKind> {
    if is_javascript_expression(json_str) {
        return Ok(());
    }

    let value = parse_wavedrom_json(json_str)?;
    let signal_array = value
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or(InvalidWavedromKind::MissingSignalArray)?;
    check_wave_chars(signal_array)
}

/// Assumes syntax is already validated by validate_wavedrom_syntax.
fn validate_wavedrom_test(
    json_str: &str,
    port_names: &[String],
) -> Result<(), InvalidWavedromKind> {
    if is_javascript_expression(json_str) {
        return Err(InvalidWavedromKind::JavaScriptInTestBlock);
    }

    let value = parse_wavedrom_json(json_str)?;
    let signal_array = value
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or(InvalidWavedromKind::MissingSignalArray)?;

    for item in signal_array {
        if let Some(obj) = item.as_object()
            && let Some(name) = obj.get("name").and_then(|v| v.as_str())
            && let Some(wave) = obj.get("wave").and_then(|v| v.as_str())
            && wave.contains('|')
        {
            return Err(InvalidWavedromKind::PipeSeparatorInTest {
                signal: name.to_string(),
            });
        }
    }

    let has_output_match = signal_array.iter().any(|item| {
        let Some(obj) = item.as_object() else {
            return false;
        };
        let Some(name) = obj.get("name").and_then(|v| v.as_str()) else {
            return false;
        };
        if let Some(wave) = obj.get("wave").and_then(|v| v.as_str())
            && wave.starts_with(['p', 'P', 'n', 'N'])
        {
            return false;
        }
        port_names
            .iter()
            .any(|port_name| port_name == name || strip_port_prefix(port_name) == name)
    });

    if !has_output_match {
        return Err(InvalidWavedromKind::NoMatchingPorts);
    }

    Ok(())
}

pub fn check_wavedrom(
    module_token: &Token,
    block: Option<WavedromBlock>,
    test_block: Option<WavedromBlock>,
    port_names: &[String],
) -> Vec<AnalyzerError> {
    let mut ret = vec![];
    let fallback_token: TokenRange = (*module_token).into();

    if let Some(block) = block
        && let Err(kind) = validate_wavedrom_syntax(&block.json)
    {
        let token = resolve_error_line(module_token, &block, &kind, fallback_token);
        ret.push(AnalyzerError::invalid_wavedrom(kind, &token));
    }

    if let Some(block) = test_block
        && let Err(kind) = validate_wavedrom_test(&block.json, port_names)
    {
        let token = resolve_error_line(module_token, &block, &kind, fallback_token);
        ret.push(AnalyzerError::invalid_wavedrom(kind, &token));
    }

    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_json5_trailing_comma_object() {
        let input = r#"{signal: [{ name: "clk", wave: "p...",},]}"#;
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn preprocess_json5_trailing_comma_array() {
        let input = r#"{signal: ["group", {name: "a", wave: "01."},]}"#;
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn preprocess_json5_trailing_comma_nested() {
        let input = r#"{signal: [{name: "clk", wave: "p...",}, {name: "data", wave: "x.=.",},],}"#;
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn preprocess_json5_trailing_comma_with_whitespace() {
        let input = "{ signal: [ { name: \"clk\", wave: \"p\" } , \n] , \n}";
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn preprocess_json5_no_trailing_comma() {
        let input = r#"{signal: [{name: "clk", wave: "p..."}]}"#;
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn preprocess_json5_single_quotes() {
        let input = "{ signal: [{ name: 'clk', wave: 'p...' }] }";
        let result = preprocess_json5(input);
        assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
    }

    #[test]
    fn javascript_expression_detected() {
        assert!(is_javascript_expression(
            "(function(bits) { return {}; })(8)",
        ));
        assert!(is_javascript_expression(
            "  (function(bits) { return {}; })(8)  ",
        ));
        assert!(!is_javascript_expression("{signal: []}"));
        assert!(!is_javascript_expression("  { signal: [] }  "));
        assert!(!is_javascript_expression("[{name: 'clk'}]"));
    }

    #[test]
    fn validate_wavedrom_syntax_accepts_js_expression() {
        let js = "(function(bits, ticks) { return {signal: []}; })(5, 16)";
        assert!(validate_wavedrom_syntax(js).is_ok());
    }

    #[test]
    fn validate_wavedrom_test_rejects_js_expression() {
        let js = "(function(bits, ticks) { return {signal: []}; })(5, 16)";
        let err = validate_wavedrom_test(js, &[]).unwrap_err();
        assert!(matches!(err, InvalidWavedromKind::JavaScriptInTestBlock));
    }
}
