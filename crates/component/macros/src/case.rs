//! Conversion of Veryl's identifiers into Rust's naming conventions.

use proc_macro2::Span;
use syn::Ident;

/// The words of an identifier, split at underscores and at case changes
/// (`HTTPServer` is `HTTP`, `Server`).
fn words(s: &str) -> Vec<String> {
    let mut ret = Vec::new();
    for part in s.split('_').filter(|x| !x.is_empty()) {
        let chars: Vec<char> = part.chars().collect();
        let mut word = String::new();
        for (i, c) in chars.iter().enumerate() {
            let boundary = i > 0 && {
                let prev = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|x| x.is_lowercase());
                (c.is_uppercase() && (prev.is_lowercase() || prev.is_ascii_digit()))
                    || (c.is_uppercase() && prev.is_uppercase() && next_lower)
            };
            if boundary {
                ret.push(std::mem::take(&mut word));
            }
            word.push(*c);
        }
        ret.push(word);
    }
    ret
}

/// `UpperCamelCase`: types, enum variants.
pub fn upper_camel(s: &str) -> String {
    words(s)
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c
                    .to_uppercase()
                    .chain(chars.flat_map(|c| c.to_lowercase()))
                    .collect(),
                None => String::new(),
            }
        })
        .collect()
}

/// `snake_case`: modules, fields.
pub fn snake(s: &str) -> String {
    words(s)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// `SCREAMING_SNAKE_CASE`: constants.
pub fn screaming_snake(s: &str) -> String {
    words(s)
        .iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// An identifier for `name`: a raw identifier for a keyword, and an
/// underscore appended to the keywords no raw identifier can be.
pub fn ident(name: &str) -> Ident {
    if syn::parse_str::<Ident>(name).is_ok() {
        return Ident::new(name, Span::call_site());
    }
    match name {
        "self" | "Self" | "super" | "crate" | "_" => {
            Ident::new(&format!("{name}_"), Span::call_site())
        }
        _ => Ident::new_raw(name, Span::call_site()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(upper_camel("bus_config"), "BusConfig");
        assert_eq!(upper_camel("BusConfig"), "BusConfig");
        assert_eq!(upper_camel("IDLE"), "Idle");
        assert_eq!(upper_camel("HTTPServer"), "HttpServer");
        assert_eq!(upper_camel("master"), "Master");
        assert_eq!(snake("dataValid"), "data_valid");
        assert_eq!(snake("i_clk"), "i_clk");
        assert_eq!(snake("BusVipPkg"), "bus_vip_pkg");
        assert_eq!(snake("addr2Reg"), "addr2_reg");
        assert_eq!(screaming_snake("maxCount"), "MAX_COUNT");
        assert_eq!(screaming_snake("REGS"), "REGS");
    }

    #[test]
    fn keywords() {
        assert_eq!(ident("type").to_string(), "r#type");
        assert_eq!(ident("self").to_string(), "self_");
        assert_eq!(ident("data").to_string(), "data");
    }
}
