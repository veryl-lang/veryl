//! Minimal wasm binary reader extracting custom sections (component
//! manifests, source hashes) from prebuilt component binaries, without a
//! wasm runtime dependency.

/// Returns the payload of the first custom section named `name`, or `None`
/// if the input is not a wasm binary or has no such section.
pub fn wasm_custom_section<'a>(wasm: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let rest = wasm.strip_prefix(b"\0asm")?;
    let mut rest = rest.strip_prefix(&[1, 0, 0, 0])?;
    while !rest.is_empty() {
        let id = rest[0];
        let (size, r) = leb128_u32(&rest[1..])?;
        let size = size as usize;
        if r.len() < size {
            return None;
        }
        let (section, tail) = r.split_at(size);
        rest = tail;
        if id != 0 {
            continue;
        }
        let (name_len, s) = leb128_u32(section)?;
        let name_len = name_len as usize;
        if s.len() < name_len {
            return None;
        }
        let (section_name, payload) = s.split_at(name_len);
        if section_name == name.as_bytes() {
            return Some(payload);
        }
    }
    None
}

/// Appends a custom section to a wasm binary (custom sections may appear
/// anywhere, including after the data section, so appending is valid).
pub fn append_wasm_custom_section(wasm: &mut Vec<u8>, name: &str, payload: &[u8]) {
    let mut body = leb128_encode(name.len() as u32);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);
    wasm.push(0);
    wasm.extend_from_slice(&leb128_encode(body.len() as u32));
    wasm.extend_from_slice(&body);
}

fn leb128_encode(mut value: u32) -> Vec<u8> {
    let mut out = vec![];
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn leb128_u32(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let mut result: u64 = 0;
    let mut i = 0;
    loop {
        let b = *bytes.get(i)?;
        result |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some((u32::try_from(result).ok()?, &bytes[i + 1..]));
        }
        i += 1;
        if i == 5 {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut body = vec![u8::try_from(name.len()).unwrap()];
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(payload);
        let mut out = vec![0u8];
        // Two-byte LEB128 even for small sizes, to exercise continuation.
        out.push(u8::try_from(body.len() & 0x7f).unwrap() | 0x80);
        out.push(u8::try_from(body.len() >> 7).unwrap());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn finds_named_section() {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        // A non-custom section is skipped.
        wasm.extend_from_slice(&[1, 2, 0x60, 0]);
        wasm.extend_from_slice(&custom_section("other", b"nope"));
        wasm.extend_from_slice(&custom_section("veryl.manifest", b"{\"types\":{}}"));

        assert_eq!(
            wasm_custom_section(&wasm, "veryl.manifest"),
            Some(&b"{\"types\":{}}"[..])
        );
        assert_eq!(wasm_custom_section(&wasm, "other"), Some(&b"nope"[..]));
        assert_eq!(wasm_custom_section(&wasm, "missing"), None);
    }

    #[test]
    fn width_expressions_evaluate() {
        use crate::{eval_width_expr, parse_width_expr};
        let params = vec![
            ("WIDTH".to_string(), 32),
            ("DEPTH".to_string(), 4),
            ("axi.DATA_WIDTH_BYTES".to_string(), 16),
        ];
        let eval = |json: &str| {
            let v: serde_json::Value = serde_json::from_str(json).unwrap();
            parse_width_expr(&v).and_then(|e| eval_width_expr(&e, &params))
        };
        assert_eq!(eval("128"), Some(128));
        assert_eq!(eval(r#""WIDTH""#), Some(32));
        assert_eq!(
            eval(r#"{"op":"+","lhs":{"op":"*","lhs":"WIDTH","rhs":2},"rhs":8}"#),
            Some(72)
        );
        assert_eq!(
            eval(r#"{"op":"*","lhs":{"op":"+","lhs":"WIDTH","rhs":"DEPTH"},"rhs":2}"#),
            Some(72)
        );
        assert_eq!(
            eval(r#"{"op":"-","lhs":{"op":"/","lhs":"WIDTH","rhs":"DEPTH"},"rhs":1}"#),
            Some(7)
        );
        // A dotted name is a group-qualified interface parameter, keyed
        // "group.name" in the environment.
        assert_eq!(
            eval(r#"{"op":"*","lhs":"axi.DATA_WIDTH_BYTES","rhs":8}"#),
            Some(128)
        );
        // Unknown parameter and unsigned underflow yield None.
        assert_eq!(eval(r#""UNKNOWN""#), None);
        assert_eq!(eval(r#""axi.UNKNOWN""#), None);
        assert_eq!(eval(r#"{"op":"-","lhs":1,"rhs":2}"#), None);
        // Malformed structures fail to parse.
        let missing_rhs: serde_json::Value = serde_json::from_str(r#"{"op":"+","lhs":1}"#).unwrap();
        assert_eq!(parse_width_expr(&missing_rhs), None);
        let not_an_expr: serde_json::Value = serde_json::from_str("[1, 2]").unwrap();
        assert_eq!(parse_width_expr(&not_an_expr), None);
    }

    #[test]
    fn width_expr_display_round_trips() {
        use crate::parse_width_expr;
        let show = |json: &str| {
            parse_width_expr(&serde_json::from_str(json).unwrap())
                .unwrap()
                .to_string()
        };
        assert_eq!(
            show(r#"{"op":"+","lhs":{"op":"*","lhs":"WIDTH","rhs":2},"rhs":8}"#),
            "WIDTH * 2 + 8"
        );
        assert_eq!(
            show(r#"{"op":"*","lhs":{"op":"+","lhs":"WIDTH","rhs":"DEPTH"},"rhs":2}"#),
            "(WIDTH + DEPTH) * 2"
        );
        assert_eq!(
            show(r#"{"op":"*","lhs":"axi.DATA_WIDTH_BYTES","rhs":8}"#),
            "axi.DATA_WIDTH_BYTES * 8"
        );
    }

    #[test]
    fn manifest_parses_from_wasm_binary() {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        let json = r#"{"types":{"iss":{"kind":"clocked","requires":["file"],"methods":[{"name":"load","args":[{"name":"path","type":"str"}]}]}}}"#;
        append_wasm_custom_section(&mut wasm, "veryl.manifest", json.as_bytes());

        let manifests = crate::ComponentManifest::parse_all_from_wasm(&wasm).unwrap();
        let manifest = &manifests["iss"];
        assert_eq!(manifest.kind.as_deref(), Some("clocked"));
        assert_eq!(manifest.requires, ["file"]);
        assert_eq!(manifest.method("load").unwrap().args[0].name, "path");
        assert!(!manifests.contains_key("other"));
    }

    #[test]
    fn appended_section_is_found() {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.extend_from_slice(&[1, 2, 0x60, 0]);
        let hash = "a".repeat(200);
        append_wasm_custom_section(&mut wasm, "veryl.source_hash", hash.as_bytes());
        assert_eq!(
            wasm_custom_section(&wasm, "veryl.source_hash"),
            Some(hash.as_bytes())
        );
    }

    #[test]
    fn rejects_non_wasm_and_truncated() {
        assert_eq!(wasm_custom_section(b"ELF", "x"), None);
        assert_eq!(wasm_custom_section(b"\0asm\x02\0\0\0", "x"), None);
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.extend_from_slice(&[0, 200]);
        assert_eq!(wasm_custom_section(&wasm, "x"), None);
    }
}
