mod parser {
    use crate::veryl_grammar::VerylGrammar;
    use crate::veryl_parser::parse;
    use parol_runtime::miette::IntoDiagnostic;
    use std::fs;

    fn test(name: &str) {
        let file = format!("testcases/{}.vl", name);
        let input = fs::read_to_string(&file).into_diagnostic().unwrap();
        let mut grammar = VerylGrammar::new();
        let ret = parse(&input, &file, &mut grammar);
        match ret {
            Ok(_) => assert!(true),
            Err(err) => println!("{}", err),
        }
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

mod formatter {
    use crate::formatter::Formatter;
    use crate::veryl_grammar::VerylGrammar;
    use crate::veryl_parser::parse;
    use parol_runtime::miette::IntoDiagnostic;
    use std::fs;

    fn test(name: &str) {
        let file = format!("testcases/{}.vl", name);
        let input = fs::read_to_string(&file).into_diagnostic().unwrap();
        let original = input.clone();

        let input = input.replace(" ", "");

        let mut grammar = VerylGrammar::new();
        let _ = parse(&input, &file, &mut grammar);
        let veryl = grammar.veryl.unwrap();
        let mut formatter = Formatter::new();

        formatter.format(&veryl);
        assert_eq!(original, formatter.string);
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}
