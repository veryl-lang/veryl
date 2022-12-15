mod formatter {
    use crate::formatter::Formatter;
    use std::fs;
    use veryl_parser::veryl_grammar::VerylGrammar;
    use veryl_parser::veryl_parser::parse;

    fn test(name: &str) {
        let file = format!("../../testcases/{}.vl", name);
        let input = fs::read_to_string(&file).unwrap();
        let original = input.clone();

        let input = input.replace(" ", "");

        let mut grammar = VerylGrammar::new();
        let _ = parse(&input, &file, &mut grammar);
        let veryl = grammar.veryl.unwrap();
        let mut formatter = Formatter::new();

        formatter.format(&veryl);
        assert_eq!(original, formatter.as_str());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}
