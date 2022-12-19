#[cfg(test)]
mod parser {
    use std::fs;
    use veryl_parser::parser::Parser;

    fn test(name: &str) {
        let file = format!("../../testcases/{}.vl", name);
        let input = fs::read_to_string(&file).unwrap();
        let ret = Parser::parse(&input, &file);
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
    use veryl_parser::parser::Parser;

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

        let ret = Parser::parse(&input, &file).unwrap();
        let mut formatter = Formatter::new();
        formatter.format(&ret.veryl);

        assert_eq!(original, formatter.as_str());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}
