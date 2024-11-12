#[cfg(test)]
const DEPENDENCY_TESTS: [&str; 2] = ["25_dependency", "68_std"];

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

        if crate::DEPENDENCY_TESTS.contains(&name) {
            let paths = metadata.paths::<&str>(&[], false).unwrap();
            let cache_path = veryl_path::cache_path().canonicalize().unwrap();
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

        Analyzer::analyze_post_pass1();

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
    use std::path::PathBuf;
    use veryl_analyzer::Analyzer;
    use veryl_emitter::Emitter;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        if crate::DEPENDENCY_TESTS.contains(&name) {
            let paths = metadata.paths::<&str>(&[], false).unwrap();
            let cache_path = veryl_path::cache_path().canonicalize().unwrap();
            for path in paths {
                if path.src.starts_with(&cache_path) {
                    let input = fs::read_to_string(&path.src).unwrap();
                    let ret = Parser::parse(&input, &path.src).unwrap();
                    let analyzer = Analyzer::new(&metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &input, &path.src, &ret.veryl);
                }
            }
        }

        let src_path = PathBuf::from(format!("../../testcases/veryl/{}.veryl", name));
        let dst_path = PathBuf::from(format!("../../testcases/sv/{}.sv", name));
        let map_path = PathBuf::from(format!("../../testcases/map/testcases/sv/{}.sv.map", name));

        let input = fs::read_to_string(&src_path).unwrap();
        let ret = Parser::parse(&input, &src_path).unwrap();
        let prj = &metadata.project.name;
        let analyzer = Analyzer::new(&metadata);
        let _ = analyzer.analyze_pass1(&prj, &input, &src_path, &ret.veryl);
        Analyzer::analyze_post_pass1();
        let _ = analyzer.analyze_pass2(&prj, &input, &src_path, &ret.veryl);
        let mut emitter = Emitter::new(&metadata, &src_path, &dst_path, &map_path);
        emitter.emit(&prj, &ret.veryl);

        let out_code = emitter.as_str();
        let ref_code = fs::read_to_string(&dst_path).unwrap();

        assert_eq!(ref_code, out_code);

        let out_map = String::from_utf8(emitter.source_map().to_bytes().unwrap()).unwrap();
        let ref_map = if cfg!(target_os = "windows") {
            fs::read_to_string(&map_path)
                .unwrap()
                .replace("\\n", "\\r\\n")
        } else {
            fs::read_to_string(&map_path).unwrap()
        };

        assert_eq!(ref_map, out_map);
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod path {
    use std::path::PathBuf;
    use veryl_metadata::{Metadata, SourceMapTarget, Target};

    fn path_test(mut metadata: Metadata, src_exp: &str, dst_exp: &str, map_exp: &str) {
        let base = metadata.project_path();
        let paths = metadata.paths::<PathBuf>(&[], false).unwrap();

        for path in paths {
            if path.src.file_name().unwrap() == "01_number.veryl" {
                assert_eq!(path.src, base.join(src_exp));
                assert_eq!(path.dst, base.join(dst_exp));
                assert_eq!(path.map, base.join(map_exp));
            }
        }
    }

    #[test]
    fn source_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.target = Target::Source;
        metadata.build.sourcemap_target = SourceMapTarget::Target;

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/veryl/01_number.sv",
            "testcases/veryl/01_number.sv.map",
        );
    }

    #[test]
    fn source_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.target = Target::Source;
        metadata.build.sourcemap_target = SourceMapTarget::Directory {
            path: "testcases/map".into(),
        };

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/veryl/01_number.sv",
            "testcases/map/testcases/veryl/01_number.sv.map",
        );
    }

    #[test]
    fn directory_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.target = Target::Directory {
            path: "testcases/sv".into(),
        };
        metadata.build.sourcemap_target = SourceMapTarget::Target;

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/sv/01_number.sv",
            "testcases/sv/01_number.sv.map",
        );
    }

    #[test]
    fn directory_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.target = Target::Directory {
            path: "testcases/sv".into(),
        };
        metadata.build.sourcemap_target = SourceMapTarget::Directory {
            path: "testcases/map".into(),
        };

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/sv/01_number.sv",
            "testcases/map/testcases/sv/01_number.sv.map",
        );
    }
}

#[cfg(test)]
mod filelist {
    use std::fs;
    use std::path::PathBuf;
    use veryl_analyzer::Analyzer;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn check_list(paths: &[String], expected: &[&str]) {
        let paths: Vec<_> = paths.iter().map(|x| x.as_str()).collect();
        for x in &paths {
            assert!(expected.contains(&x));
        }
        for x in expected {
            assert!(paths.contains(x));
        }
    }

    fn check_order(paths: &[String], path0: &str, path1: &str) {
        let path0 = paths
            .iter()
            .enumerate()
            .find_map(|(i, x)| if x == path0 { Some(i) } else { None });
        let path1 = paths
            .iter()
            .enumerate()
            .find_map(|(i, x)| if x == path1 { Some(i) } else { None });
        assert!(path0 < path1);
    }

    #[test]
    fn test() {
        let path = std::env::current_dir().unwrap();
        let path = path.join("../../testcases/filelist");
        let metadata_path = Metadata::search_from(path).unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();
        let paths = metadata.paths::<PathBuf>(&[], false).unwrap();

        let mut contexts = Vec::new();

        for path in &paths {
            let input = fs::read_to_string(&path.src).unwrap();
            let parser = Parser::parse(&input, &path.src).unwrap();

            let analyzer = Analyzer::new(&metadata);
            let _ = analyzer.analyze_pass1(&path.prj, &input, &path.src, &parser.veryl);
            contexts.push((path, input, parser, analyzer));
        }

        Analyzer::analyze_post_pass1();

        for (path, input, parser, analyzer) in &contexts {
            let _ = analyzer.analyze_pass2(&path.prj, input, &path.src, &parser.veryl);
        }

        for (path, input, parser, analyzer) in &contexts {
            let _ = analyzer.analyze_pass3(&path.prj, input, &path.src, &parser.veryl);
        }

        let paths = veryl::cmd_build::CmdBuild::sort_filelist(&metadata, &paths, false);
        let paths: Vec<_> = paths
            .into_iter()
            .map(|x| x.src.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        dbg!(&paths);

        let all = &[
            "package_a.veryl",
            "package_b.veryl",
            "module_a.veryl",
            "module_b.veryl",
            "module_c.veryl",
            "ram.veryl",
        ];
        check_list(&paths, all);

        check_order(&paths, "package_a.veryl", "module_a.veryl");
        check_order(&paths, "package_b.veryl", "module_b.veryl");
        check_order(&paths, "ram.veryl", "module_c.veryl");
    }
}
