#[cfg(test)]
mod parser {
    use std::fs;
    use veryl_parser::Parser;

    fn test(name: &str) {
        let file = format!("../../testcases/veryl/{}.veryl", name);
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
mod analyzer {
    use std::fs;
    use veryl_analyzer::Analyzer;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        if name == "25_dependency" {
            let paths = metadata.paths::<&str>(&[], false).unwrap();
            let cache_path = Metadata::cache_path().canonicalize().unwrap();
            for path in paths {
                if path.src.starts_with(&cache_path) {
                    let input = fs::read_to_string(&path.src).unwrap();
                    let ret = Parser::parse(&input, &path.src).unwrap();
                    let analyzer = Analyzer::new(&metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &input, &path.src, &ret.veryl);
                }
            }
        }

        let file = format!("../../testcases/veryl/{}.veryl", name);
        let input = fs::read_to_string(&file).unwrap();

        let ret = Parser::parse(&input, &file).unwrap();
        let prj = &metadata.project.name;
        let analyzer = Analyzer::new(&metadata);
        let errors = analyzer.analyze_pass1(&prj, &input, &file, &ret.veryl);
        dbg!(&errors);
        assert!(errors.is_empty());

        let errors = analyzer.analyze_pass2(&prj, &input, &file, &ret.veryl);
        dbg!(&errors);
        assert!(errors.is_empty());

        let errors = analyzer.analyze_pass3(&prj, &input, &file, &ret.veryl);
        dbg!(&errors);
        assert!(errors.is_empty());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod formatter {
    use std::fs;
    use veryl_formatter::Formatter;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        let metadata_path = Metadata::search_from_current().unwrap();
        let metadata = Metadata::load(&metadata_path).unwrap();

        let file = format!("../../testcases/veryl/{}.veryl", name);
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
        let mut formatter = Formatter::new(&metadata);
        formatter.format(&ret.veryl);

        assert_eq!(original, formatter.as_str());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod emitter {
    use std::fs;
    use veryl_analyzer::Analyzer;
    use veryl_emitter::Emitter;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        if name == "25_dependency" {
            let paths = metadata.paths::<&str>(&[], false).unwrap();
            let cache_path = Metadata::cache_path().canonicalize().unwrap();
            for path in paths {
                if path.src.starts_with(&cache_path) {
                    let input = fs::read_to_string(&path.src).unwrap();
                    let ret = Parser::parse(&input, &path.src).unwrap();
                    let analyzer = Analyzer::new(&metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &input, &path.src, &ret.veryl);
                }
            }
        }

        let file = format!("../../testcases/veryl/{}.veryl", name);
        let input = fs::read_to_string(&file).unwrap();

        let ret = Parser::parse(&input, &file).unwrap();
        let prj = &metadata.project.name;
        let analyzer = Analyzer::new(&metadata);
        let _ = analyzer.analyze_pass1(&prj, &input, &file, &ret.veryl);
        let _ = analyzer.analyze_pass2(&prj, &input, &file, &ret.veryl);
        let mut emitter = Emitter::new(&metadata);
        emitter.emit(&prj, &ret.veryl);

        let file = format!("../../testcases/sv/{}.sv", name);
        let reference = fs::read_to_string(&file).unwrap();

        assert_eq!(reference, emitter.as_str());
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}
