use crate::AnalyzerError;
use crate::symbol::WavedromBlock;
use veryl_parser::resource_table::{self, StrId};
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

    result
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

/// WaveDrom validation error with an optional signal name for locating the source line.
struct WavedromValidationError {
    message: String,
    signal_hint: Option<String>,
}

impl WavedromValidationError {
    fn new(message: String) -> Self {
        Self {
            message,
            signal_hint: None,
        }
    }

    fn with_signal(message: String, signal: &str) -> Self {
        Self {
            message,
            signal_hint: Some(signal.to_string()),
        }
    }
}

/// Resolve error location: the specific signal line if available, otherwise the whole block.
fn resolve_error_line(
    module_token: &Token,
    block: &WavedromBlock,
    err: &WavedromValidationError,
    fallback: TokenRange,
) -> TokenRange {
    if let Some(ref hint) = err.signal_hint
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

type WavedromResult = Result<(), WavedromValidationError>;

fn parse_wavedrom_json(json_str: &str) -> Result<serde_json::Value, WavedromValidationError> {
    let json_str = preprocess_json5(json_str);
    serde_json::from_str(&json_str)
        .map_err(|e| WavedromValidationError::new(format!("invalid WaveDrom JSON: {e}")))
}

fn check_wave_chars(signal_array: &[serde_json::Value]) -> WavedromResult {
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
                    return Err(WavedromValidationError::with_signal(
                        format!("WaveDrom signal '{name}' contains unknown wave character '{c}'"),
                        name,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Validate WaveDrom JSON syntax (for all ```wavedrom blocks).
fn validate_wavedrom_syntax(json_str: &str) -> WavedromResult {
    let value = parse_wavedrom_json(json_str)?;
    let signal_array = value
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            WavedromValidationError::new("WaveDrom JSON missing 'signal' array".to_string())
        })?;
    check_wave_chars(signal_array)
}

/// Validate a ```wavedrom,test block for simulation testing.
///
/// Assumes syntax is already validated by validate_wavedrom_syntax.
/// Checks:
/// - No '|' pipe separators (indeterminate timing)
/// - At least one non-clock signal matches a module port
fn validate_wavedrom_test(json_str: &str, port_names: &[String]) -> WavedromResult {
    let value = parse_wavedrom_json(json_str)?;
    let signal_array = value
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            WavedromValidationError::new("WaveDrom JSON missing 'signal' array".to_string())
        })?;

    for item in signal_array {
        if let Some(obj) = item.as_object()
            && let Some(name) = obj.get("name").and_then(|v| v.as_str())
            && let Some(wave) = obj.get("wave").and_then(|v| v.as_str())
            && wave.contains('|')
        {
            return Err(WavedromValidationError::with_signal(
                format!(
                    "WaveDrom signal '{name}' contains '|' separator (indeterminate timing, not testable)"
                ),
                name,
            ));
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
        return Err(WavedromValidationError::new(
            "WaveDrom test has no signals matching module ports".to_string(),
        ));
    }

    Ok(())
}

/// Validate WaveDrom blocks in a module's doc comment and return any errors.
pub fn check_wavedrom(
    module_token: &Token,
    block: Option<WavedromBlock>,
    test_block: Option<WavedromBlock>,
    port_names: &[String],
) -> Vec<AnalyzerError> {
    let mut ret = vec![];
    let fallback_token: TokenRange = (*module_token).into();

    // Check all ```wavedrom blocks for syntax errors
    if let Some(block) = block
        && let Err(e) = validate_wavedrom_syntax(&block.json)
    {
        let token = resolve_error_line(module_token, &block, &e, fallback_token);
        ret.push(AnalyzerError::invalid_wavedrom(&e.message, &token));
    }

    // Check ```wavedrom,test blocks for testability
    if let Some(block) = test_block
        && let Err(e) = validate_wavedrom_test(&block.json, port_names)
    {
        let token = resolve_error_line(module_token, &block, &e, fallback_token);
        ret.push(AnalyzerError::invalid_wavedrom(&e.message, &token));
    }

    ret
}
