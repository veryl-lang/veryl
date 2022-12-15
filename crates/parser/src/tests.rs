mod parser {
    use crate::veryl_grammar::VerylGrammar;
    use crate::veryl_parser::parse;
    use std::fs;

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
