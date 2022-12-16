#[cfg(test)]
mod parser {
    use std::fs;
    use veryl_parser::veryl_grammar::VerylGrammar;
    use veryl_parser::veryl_parser::parse;

    fn test(name: &str) {
        let file = format!("../../testcases/{}.vl", name);
        let input = fs::read_to_string(&file).unwrap();
        let mut grammar = VerylGrammar::new();
        let ret = parse(&input, &file, &mut grammar);
        match ret {
            Ok(_) => assert!(true),
            Err(err) => println!("{}", err),
        }
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod formatter {
    use std::fs;
    use veryl_formatter::formatter::Formatter;
    use veryl_parser::veryl_grammar::VerylGrammar;
    use veryl_parser::veryl_parser::parse;

    fn test(name: &str) {
        let file = format!("../../testcases/{}.vl", name);
        let input = fs::read_to_string(&file).unwrap();
        let original = input.clone();

        // minify without lines which contain line comment
        let mut minified = String::new();
        for line in input.lines() {
            if line.contains("//") {
                minified.push_str(&format!("{}\n", line));
            } else {
                minified.push_str(&format!("{}\n", line.replace(' ', "")));
            }
        }

        let mut grammar = VerylGrammar::new();
        let _ = parse(&minified, &file, &mut grammar);
        let veryl = grammar.veryl.unwrap();
        let mut formatter = Formatter::new();

        formatter.format(&veryl);
        assert_eq!(original, formatter.as_str());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}
