#[cfg(not(feature = "build"))]
fn main() {}

#[cfg(feature = "build")]
fn generate_token_type() {
    use regex::Regex;
    use std::fs;
    use std::path::PathBuf;

    struct TokenDef {
        variant_name: String,
        term_name: String,
        display: String,
        is_keyword: bool,
    }

    // Display overrides for tokens without single-quoted literal patterns in veryl.par
    const DISPLAY_OVERRIDES: &[(&str, &str)] = &[
        ("Comments", "comment"),
        ("StringLiteral", "string literal"),
        ("Exponent", "number"),
        ("FixedPoint", "number"),
        ("Based", "number"),
        ("AllBit", "number"),
        ("BaseLess", "number"),
        ("AssignmentOperator", "assignment operator"),
        ("Operator08", "operator"),
        ("Operator07", "operator"),
        ("Operator06", "operator"),
        ("Operator02", "operator"),
        ("Operator01", "operator"),
        ("Operator05", "operator"),
        ("Operator04", "operator"),
        ("Operator03", "operator"),
        ("UnaryOperator", "operator"),
        ("QuoteLBrace", "'{"),
        ("Quote", "'"),
        ("DollarIdentifier", "$identifier"),
        ("Identifier", "identifier"),
        ("Any", "embed content"),
    ];

    let par_content = fs::read_to_string("veryl.par").expect("Failed to read veryl.par");

    // Only match lines with scanner specification <...> to exclude grammar rules
    let re = Regex::new(r"^(\w+)\s*:\s*<[^>]+>(?:'([^']+)'|[^\s]+)\s*:\s*Token\s*;")
        .expect("Invalid regex");
    let keyword_re = Regex::new(r":\s*Token\s*;\s*//\s*Keyword").expect("Invalid keyword regex");

    let mut tokens: Vec<TokenDef> = Vec::new();

    for line in par_content.lines() {
        if !line.contains(": Token;") {
            continue;
        }

        let line_trimmed = line.trim();
        if let Some(caps) = re.captures(line_trimmed) {
            let term_name = caps.get(1).unwrap().as_str().to_string();
            let literal = caps.get(2).map(|m| m.as_str().to_string());
            let is_keyword = keyword_re.is_match(line_trimmed);

            let variant_name = term_name
                .strip_suffix("Term")
                .unwrap_or(&term_name)
                .to_string();

            let display = if is_keyword {
                literal.clone().unwrap_or_else(|| {
                    panic!("Keyword token {} has no literal pattern", variant_name)
                })
            } else if let Some(ref lit) = literal {
                lit.clone()
            } else {
                DISPLAY_OVERRIDES
                    .iter()
                    .find(|(name, _)| *name == variant_name)
                    .unwrap_or_else(|| {
                        panic!(
                            "Token {} has no literal and no display override",
                            variant_name
                        )
                    })
                    .1
                    .to_string()
            };

            tokens.push(TokenDef {
                variant_name,
                term_name,
                display,
                is_keyword,
            });
        }
    }

    assert!(!tokens.is_empty(), "No tokens found in veryl.par");

    let mut code = String::new();

    code.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n");
    code.push_str("pub enum TokenType {\n");
    for t in &tokens {
        code.push_str(&format!("    {},\n", t.variant_name));
    }
    code.push_str("    Error,\n");
    code.push_str("}\n\n");

    let keyword_variants: Vec<&str> = tokens
        .iter()
        .filter(|t| t.is_keyword)
        .map(|t| t.variant_name.as_str())
        .collect();
    code.push_str("impl TokenType {\n");
    code.push_str("    pub fn is_keyword(&self) -> bool {\n");
    if keyword_variants.is_empty() {
        code.push_str("        false\n");
    } else {
        code.push_str("        matches!(\n");
        code.push_str("            self,\n");
        for (i, v) in keyword_variants.iter().enumerate() {
            if i == 0 {
                code.push_str(&format!("            TokenType::{}", v));
            } else {
                code.push_str(&format!("\n                | TokenType::{}", v));
            }
        }
        code.push('\n');
        code.push_str("        )\n");
    }
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("impl From<&str> for TokenType {\n");
    code.push_str("    fn from(value: &str) -> Self {\n");
    code.push_str("        match value {\n");
    for t in &tokens {
        code.push_str(&format!(
            "            \"{}\" => TokenType::{},\n",
            t.term_name, t.variant_name
        ));
    }
    code.push_str("            _ => TokenType::Error,\n");
    code.push_str("        }\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("impl std::fmt::Display for TokenType {\n");
    code.push_str("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n");
    code.push_str("        let text = match self {\n");
    for t in &tokens {
        let escaped = t.display.replace('\\', "\\\\").replace('"', "\\\"");
        code.push_str(&format!(
            "            TokenType::{} => \"{}\",\n",
            t.variant_name, escaped
        ));
    }
    code.push_str("            TokenType::Error => \"error\",\n");
    code.push_str("        };\n");
    code.push_str("        text.fmt(f)\n");
    code.push_str("    }\n");
    code.push_str("}\n");

    let out_path = PathBuf::from("src/generated/token_type_generated.rs");
    fs::write(&out_path, code).expect("Failed to write generated file");
}

#[cfg(feature = "build")]
fn main() {
    use parol::parol_runtime::Report;
    use parol::{ParolErrorReporter, build::Builder};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::time::Instant;

    // Skip in GitHub Actions
    if let Ok(x) = env::var("GITHUB_ACTIONS")
        && x == "true"
    {
        return;
    }

    let generate_parser = if env::var("VERYL_GENERATE_PARSER").is_ok() {
        true
    } else {
        let par_file = PathBuf::from("veryl.par");
        let exp_file = PathBuf::from("src/generated/veryl-exp.par");

        let par_modified = fs::metadata(par_file).unwrap().modified().unwrap();
        let exp_modified = fs::metadata(exp_file).unwrap().modified().unwrap();

        par_modified > exp_modified
    };

    if generate_parser {
        println!("cargo:warning=veryl.par was changed");
        generate_token_type();

        let now = Instant::now();

        // CLI equivalent is:
        // parol -f ./veryl.par -e ./veryl-exp.par -p ./src/veryl_parser.rs -a ./src/veryl_grammar_trait.rs -t VerylGrammar -m veryl_grammar
        if let Err(err) = Builder::with_explicit_output_dir("src/generated")
            .grammar_file("veryl.par")
            .expanded_grammar_output_file("veryl-exp.par")
            .parser_output_file("veryl_parser.rs")
            .actions_output_file("veryl_grammar_trait.rs")
            .user_type_name("VerylGrammar")
            .user_trait_module_name("veryl_grammar")
            .trim_parse_tree()
            .generate_parser()
        {
            {
                ParolErrorReporter::report_error(&err, "veryl.par").unwrap_or_default();
                process::exit(1);
            }
        }

        let elapsed_time = now.elapsed();
        println!(
            "cargo:warning=parol build time: {} milliseconds",
            elapsed_time.as_millis()
        );
    }
}
