#[cfg(test)]
const DEPENDENCY_TESTS: [&str; 2] = ["25_dependency_1", "25_dependency_2"];

#[cfg(test)]
const STD_TESTS: [&str; 1] = ["68_std"];

#[cfg(test)]
static DEPENDENCY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    use veryl_analyzer::{Analyzer, Context};
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        if crate::DEPENDENCY_TESTS.contains(&name) && crate::DEPENDENCY_TESTS[0] != name {
            return;
        }

        let needs_sub_project =
            crate::DEPENDENCY_TESTS.contains(&name) || crate::STD_TESTS.contains(&name);
        let _lock = if needs_sub_project {
            Some(crate::DEPENDENCY_LOCK.lock())
        } else {
            None
        };

        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        if needs_sub_project {
            let paths = metadata.paths::<&str>(&[], false, true).unwrap();
            let dependency_path = metadata.project_dependencies_path();
            for path in paths {
                if path.dst.starts_with(&dependency_path) {
                    let input = fs::read_to_string(&path.src).unwrap();
                    let ret = Parser::parse(&input, &path.src).unwrap();
                    let analyzer = Analyzer::new(&metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &ret.veryl);
                }
            }
        }

        let files = if crate::DEPENDENCY_TESTS.contains(&name) {
            crate::DEPENDENCY_TESTS.to_vec()
        } else {
            vec![name]
        };

        let mut context = Context::default();
        for file in &files {
            let file = format!("../../testcases/veryl/{}.veryl", file);
            let input = fs::read_to_string(&file).unwrap();

            let ret = Parser::parse(&input, &file).unwrap();
            let prj = &metadata.project.name;
            let analyzer = Analyzer::new(&metadata);
            let errors = analyzer.analyze_pass1(&prj, &ret.veryl);
            dbg!(&errors);
            assert!(errors.is_empty());

            let errors = Analyzer::analyze_post_pass1();
            dbg!(&errors);
            assert!(errors.is_empty());

            let errors = analyzer.analyze_pass2(&prj, &ret.veryl, &mut context, None);
            dbg!(&errors);
            assert!(errors.is_empty());

            let errors = Analyzer::analyze_post_pass2();
            dbg!(&errors);
            assert!(errors.is_empty());
        }
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod formatter {
    use std::fs;
    use veryl_analyzer::Analyzer;
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
        let prj = &metadata.project.name;
        let analyzer = Analyzer::new(&metadata);
        let _ = analyzer.analyze_pass1(&prj, &ret.veryl);
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
    use veryl_analyzer::{Analyzer, Context};
    use veryl_emitter::Emitter;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    fn test(name: &str) {
        if crate::DEPENDENCY_TESTS.contains(&name) && crate::DEPENDENCY_TESTS[0] != name {
            return;
        }

        let needs_sub_project =
            crate::DEPENDENCY_TESTS.contains(&name) || crate::STD_TESTS.contains(&name);
        let _lock = if needs_sub_project {
            Some(crate::DEPENDENCY_LOCK.lock())
        } else {
            None
        };

        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        if needs_sub_project {
            let paths = metadata.paths::<&str>(&[], false, true).unwrap();
            let dependency_path = metadata.project_dependencies_path();
            for path in paths {
                if path.dst.starts_with(&dependency_path) {
                    let input = fs::read_to_string(&path.src).unwrap();
                    let ret = Parser::parse(&input, &path.src).unwrap();
                    let analyzer = Analyzer::new(&metadata);
                    let _ = analyzer.analyze_pass1(&path.prj, &ret.veryl);
                }
            }
        }

        let names = if crate::DEPENDENCY_TESTS.contains(&name) {
            crate::DEPENDENCY_TESTS.to_vec()
        } else {
            vec![name]
        };

        let file_paths: Vec<_> = names
            .iter()
            .map(|name| {
                let src_path = PathBuf::from(format!("../../testcases/veryl/{}.veryl", name));
                let dst_path = PathBuf::from(format!("../../testcases/sv/{}.sv", name));
                let map_path = PathBuf::from(format!("../../testcases/map/{}.sv.map", name));
                (src_path, dst_path, map_path)
            })
            .collect();

        let parse_results: Vec<_> = file_paths
            .iter()
            .map(|(src, _, _)| {
                let input = fs::read_to_string(&src).unwrap();
                Parser::parse(&input, &src).unwrap()
            })
            .collect();

        let mut context = Context::default();
        for result in &parse_results {
            let prj = &metadata.project.name;
            let analyzer = Analyzer::new(&metadata);
            let _ = analyzer.analyze_pass1(&prj, &result.veryl);
            let _ = Analyzer::analyze_post_pass1();
            let _ = analyzer.analyze_pass2(&prj, &result.veryl, &mut context, None);
        }

        for (i, result) in parse_results.iter().enumerate() {
            let (src, dst, map) = &file_paths[i];
            let prj = &metadata.project.name;

            let mut emitter = Emitter::new(&metadata, src, dst, map);
            emitter.emit(&prj, &result.veryl);

            let out_code = emitter.as_str();
            let ref_code = fs::read_to_string(dst).unwrap();

            assert_eq!(ref_code, out_code);

            let out_map = String::from_utf8(emitter.source_map().to_bytes().unwrap()).unwrap();
            let ref_map = if cfg!(target_os = "windows") {
                fs::read_to_string(map).unwrap().replace("\\n", "\\r\\n")
            } else {
                fs::read_to_string(map).unwrap()
            };

            assert_eq!(ref_map, out_map);
        }
    }

    include!(concat!(env!("OUT_DIR"), "/test.rs"));
}

#[cfg(test)]
mod path {
    use std::path::PathBuf;
    use veryl_metadata::{Metadata, SourceMapTarget, Target};

    fn path_test(mut metadata: Metadata, src_exp: &str, dst_exp: &str, map_exp: &str) {
        let base = metadata.project_path();
        let paths = metadata.paths::<PathBuf>(&[], false, true).unwrap();

        for path in paths {
            if path.src.file_name().unwrap() == "01_number.veryl" {
                assert_eq!(path.src, base.join(src_exp));
                assert_eq!(path.dst, base.join(dst_exp));
                assert_eq!(path.map, base.join(map_exp));
            }
        }
    }

    #[test]
    fn rootdir_source_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("")];
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
    fn rootdir_source_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("")];
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
    fn rootdir_directory_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("")];
        metadata.build.target = Target::Directory {
            path: "testcases/sv".into(),
        };
        metadata.build.sourcemap_target = SourceMapTarget::Target;

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/sv/testcases/veryl/01_number.sv",
            "testcases/sv/testcases/veryl/01_number.sv.map",
        );
    }

    #[test]
    fn rootdir_directory_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("")];
        metadata.build.target = Target::Directory {
            path: "testcases/sv".into(),
        };
        metadata.build.sourcemap_target = SourceMapTarget::Directory {
            path: "testcases/map".into(),
        };

        path_test(
            metadata,
            "testcases/veryl/01_number.veryl",
            "testcases/sv/testcases/veryl/01_number.sv",
            "testcases/map/testcases/veryl/01_number.sv.map",
        );
    }

    #[test]
    fn subdir_source_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("testcases/veryl/")];
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
    fn subdir_source_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("testcases/veryl/")];
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
    fn subdir_directory_target() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("testcases/veryl/")];
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
    fn subdir_directory_directory() {
        let metadata_path = Metadata::search_from_current().unwrap();
        let mut metadata = Metadata::load(&metadata_path).unwrap();

        metadata.build.sources = vec![PathBuf::from("testcases/veryl/")];
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
            "testcases/map/01_number.sv.map",
        );
    }
}

#[cfg(test)]
mod filelist {
    use std::fs;
    use std::path::PathBuf;
    use veryl_analyzer::{Analyzer, Context};
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
        let paths = metadata.paths::<PathBuf>(&[], false, true).unwrap();

        let mut contexts = Vec::new();

        for path in &paths {
            let input = fs::read_to_string(&path.src).unwrap();
            let parser = Parser::parse(&input, &path.src).unwrap();

            let analyzer = Analyzer::new(&metadata);
            let _ = analyzer.analyze_pass1(&path.prj, &parser.veryl);
            contexts.push((path, input, parser, analyzer));
        }

        let err = Analyzer::analyze_post_pass1();
        dbg!(&err);
        assert!(err.is_empty());

        let mut context = Context::default();
        for (path, _, parser, analyzer) in &contexts {
            let err = analyzer.analyze_pass2(&path.prj, &parser.veryl, &mut context, None);
            dbg!(&err);
            assert!(err.is_empty());
        }

        let err = Analyzer::analyze_post_pass2();
        dbg!(&err);
        assert!(err.is_empty());

        let paths = veryl::cmd_build::CmdBuild::sort_filelist(&metadata, &paths, false);
        let paths: Vec<_> = paths
            .into_iter()
            .map(|x| x.src.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        dbg!(&paths);

        let all = &[
            "01_package_a.veryl",
            "02_package_b.veryl",
            "03_module_a.veryl",
            "04_module_b.veryl",
            "05_module_c.veryl",
            "06_package_c.veryl",
            "07_module_d.veryl",
            "08_module_e.veryl",
            "09_module_f.veryl",
            "10_package_g.veryl",
            "11_package_h.veryl",
            "12_alias_i.veryl",
            "13_embed.veryl",
            "14_package_j.veryl",
            "15_module_k.veryl",
            "16_package_l.veryl",
            "17_package_m.veryl",
            "18_package_n.veryl",
            "19_package_o.veryl",
            "20_module_p.veryl",
            "21_alias_q.veryl",
            "ram.veryl",
            "axi_pkg.veryl",
        ];
        check_list(&paths, all);

        check_order(&paths, "01_package_a.veryl", "03_module_a.veryl");
        check_order(&paths, "02_package_b.veryl", "04_module_b.veryl");
        check_order(&paths, "ram.veryl", "05_module_c.veryl");
        check_order(&paths, "06_package_c.veryl", "07_module_d.veryl");
        check_order(&paths, "07_module_d.veryl", "09_module_f.veryl");
        check_order(&paths, "09_module_f.veryl", "08_module_e.veryl");
        check_order(&paths, "10_package_g.veryl", "11_package_h.veryl");
        check_order(&paths, "axi_pkg.veryl", "12_alias_i.veryl");
        check_order(&paths, "axi_pkg.veryl", "13_embed.veryl");
        check_order(&paths, "14_package_j.veryl", "16_package_l.veryl");
        check_order(&paths, "16_package_l.veryl", "15_module_k.veryl");
        check_order(&paths, "17_package_m.veryl", "18_package_n.veryl");
        check_order(&paths, "18_package_n.veryl", "19_package_o.veryl");
        check_order(&paths, "19_package_o.veryl", "20_module_p.veryl");
        check_order(&paths, "20_module_p.veryl", "21_alias_q.veryl");
    }
}
