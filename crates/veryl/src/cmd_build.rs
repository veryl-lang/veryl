use crate::OptBuild;
use crate::StopWatch;
use crate::cmd_check::CheckError;
use crate::context::Context;
use crate::diff::print_diff;
use crate::incremental::Incremental;
use crate::utils;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::{Analyzer, symbol_table, type_dag};
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, SourceMapTarget, Target};
use veryl_parser::resource_table::PathId;
use veryl_parser::{Parser, resource_table, veryl_token::TokenSource};
use veryl_path::PathSet;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(
        &self,
        metadata: &mut Metadata,
        include_tests: bool,
        quiet: bool,
        mut ir: Option<&mut veryl_analyzer::ir::Ir>,
        test_filter: Option<&str>,
        defines: &[String],
    ) -> Result<bool> {
        if let Some(ref out_dir) = self.opt.out_dir {
            let out_dir = if out_dir.is_absolute() {
                out_dir.clone()
            } else {
                std::env::current_dir()
                    .into_diagnostic()
                    .wrap_err("")?
                    .join(out_dir)
            };
            std::fs::create_dir_all(&out_dir).into_diagnostic()?;
            metadata.output_dir_override = Some(out_dir.canonicalize().into_diagnostic()?);
        }

        let paths = metadata.paths(&self.opt.files, true, true)?;

        let mut check_error = CheckError::new(metadata.build.error_count_limit);
        let mut contexts = Vec::new();

        let mut stopwatch = StopWatch::new();

        // The fragment cache replaces parse + pass1 for unchanged files,
        // which therefore never reach pass2/emit. A caller-supplied IR
        // needs pass2 of every file, so the cache stays off there.
        let mut incremental = if ir.is_none() {
            Incremental::open(metadata, &paths, defines)
        } else {
            None
        };

        let analyzer = Analyzer::new(metadata);

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            if let Some(x) = incremental.as_mut()
                && x.try_restore(path)
            {
                continue;
            }

            let input = match incremental.as_mut().and_then(|x| x.take_input(&path.src)) {
                Some(x) => x,
                None => fs::read_to_string(&path.src)
                    .into_diagnostic()
                    .wrap_err("")?,
            };

            let watermark = incremental
                .as_ref()
                .map(|_| veryl_analyzer::fragment_cache::watermark());
            let parser = Parser::parse(&input, &path.src)?;

            let mut errors = analyzer.analyze_pass1(&path.prj, &parser.veryl);
            if let (Some(x), Some(watermark)) = (incremental.as_mut(), watermark.as_ref()) {
                x.capture(path, &input, watermark, errors.is_empty());
            }
            check_error = check_error.append(&mut errors).check_err()?;

            let context = Context::new(path.clone(), input, parser, analyzer.clone())?;
            contexts.push(context);
        }

        if let Some(x) = incremental.as_ref() {
            info!("Restored {}/{} files from cache", x.restored, paths.len());
        }
        debug!(
            "Executed parse/analyze_pass1 ({} milliseconds, {} files)",
            stopwatch.lap(),
            paths.len(),
        );

        let mut errors = Analyzer::analyze_post_pass1();
        check_error = check_error.append(&mut errors).check_err()?;

        debug!(
            "Executed analyze_post_pass1 ({} milliseconds)",
            stopwatch.lap()
        );

        if metadata.build.incremental && metadata.build_info.veryl_version_match() {
            Self::check_skip(metadata, &mut contexts);
        }

        // Testbench files whose tests don't match `--test` will never be
        // simulated, so skip pass2/emit for them.
        if let Some(filter) = test_filter {
            let tests = veryl_analyzer::symbol_table::get_tests(&metadata.project.name);
            let mut test_file_ids: HashSet<PathId> = HashSet::new();
            let mut matching_file_ids: HashSet<PathId> = HashSet::new();
            for (name, prop) in &tests {
                test_file_ids.insert(prop.path);
                let name = name.to_string();
                if name.contains(filter) {
                    matching_file_ids.insert(prop.path);
                }
            }
            let mut skipped = 0usize;
            for context in contexts.iter_mut() {
                if context.skip {
                    continue;
                }
                let path_id = resource_table::insert_path(&context.path.src);
                if test_file_ids.contains(&path_id) && !matching_file_ids.contains(&path_id) {
                    context.skip = true;
                    skipped += 1;
                }
            }
            debug!(
                "test filter {:?}: skipped {} non-matching testbench files",
                filter, skipped
            );
        }

        let mut analyzer_context = veryl_analyzer::Context::default();

        for name in defines {
            analyzer_context
                .config
                .defines
                .insert(resource_table::insert_str(name));
        }
        analyzer_context.enable_conv_profiler();

        // Build a local IR when the caller didn't supply one, so the
        // post-pass2 combinational-loop check has something to inspect.
        // This is cheap relative to the rest of pass2 because most of
        // the work is recomputing AssignTable / FfTable / per_decl_refs
        // which we share with the IR build anyway.
        let mut local_ir = veryl_analyzer::ir::Ir::default();
        let ir_for_pass2: &mut veryl_analyzer::ir::Ir = match ir {
            Some(ref mut x) => x,
            None => &mut local_ir,
        };
        for context in &contexts {
            if !context.skip {
                let path = &context.path;
                analyzer_context.set_project_name(&path.prj);
                let mut errors = context.analyzer.analyze_pass2(
                    &path.prj,
                    &context.parser.veryl,
                    &mut analyzer_context,
                    Some(ir_for_pass2),
                );
                check_error = check_error.append(&mut errors).check_err()?;
            }
        }

        debug!("Executed analyze_pass2 ({} milliseconds)", stopwatch.lap());
        analyzer_context.finalize_conv_profiler()?;

        let mut errors = Analyzer::analyze_post_pass2(ir_for_pass2);
        check_error = check_error.append(&mut errors).check_err()?;

        debug!(
            "Executed analyze_post_pass2 ({} milliseconds)",
            stopwatch.lap()
        );

        let temp_dir = if let Target::Bundle { .. } = &metadata.build.target {
            Some(TempDir::new().into_diagnostic()?)
        } else {
            None
        };

        let mut all_pass = true;
        for context in contexts.drain(..) {
            if !context.skip {
                let path = &context.path;
                let (dst, map) = if let Some(ref temp_dir) = temp_dir {
                    let output_dir = metadata.output_dir();
                    let dst_temp = temp_dir
                        .path()
                        .join(path.dst.strip_prefix(&output_dir).into_diagnostic()?);
                    let map_temp = temp_dir
                        .path()
                        .join(path.map.strip_prefix(&output_dir).into_diagnostic()?);
                    (dst_temp, map_temp)
                } else {
                    (path.dst.clone(), path.map.clone())
                };

                let mut emitter = Emitter::new(metadata, &path.src, &dst, &map);
                emitter.emit(&path.prj, &context.parser.veryl, &context.input);

                let dst_dir = dst.parent().unwrap();
                if !dst_dir.exists() {
                    std::fs::create_dir_all(dst.parent().unwrap()).into_diagnostic()?;
                }

                let exclude_check = context.path.prj == "$std";

                if self.opt.check && !exclude_check {
                    let output = fs::read_to_string(&dst).unwrap_or(String::new());
                    if output != emitter.as_str() {
                        if !quiet {
                            print_diff(&path.src, &output, emitter.as_str());
                        }
                        all_pass = false;
                    }
                } else {
                    let written = utils::write_file_if_changed(&dst, emitter.as_str().as_bytes())?;
                    if written {
                        debug!("Output file ({})", dst.to_string_lossy());
                    }

                    metadata.add_generated_file(dst);

                    if metadata.build.sourcemap_target != SourceMapTarget::None {
                        let source_map = emitter.source_map();
                        source_map.set_source_content(&context.input);
                        let source_map = source_map.to_bytes().into_diagnostic()?;

                        let map_dir = map.parent().unwrap();
                        if !map_dir.exists() {
                            std::fs::create_dir_all(map.parent().unwrap()).into_diagnostic()?;
                        }

                        let written = utils::write_file_if_changed(&map, &source_map)?;
                        if written {
                            debug!("Output map ({})", map.to_string_lossy());
                        }

                        metadata.add_generated_file(map);
                    }
                }
            }
            // context (including parser AST and input string) is dropped here
        }

        debug!("Executed emit ({} milliseconds)", stopwatch.lap());

        if !self.opt.check {
            self.gen_filelist(metadata, &paths, temp_dir, include_tests)?;
        }

        debug!("Executed filelist ({} milliseconds)", stopwatch.lap());

        let _ = check_error.check_err()?;

        if let Some(x) = incremental.as_mut() {
            x.save();
            debug!("Saved fragment cache ({} milliseconds)", stopwatch.lap());
        }

        Ok(all_pass)
    }

    fn gen_filelist_line(&self, metadata: &Metadata, path: &Path) -> Result<String> {
        let base_path = metadata.output_dir();
        let path = path.canonicalize().into_diagnostic()?;
        Ok(match metadata.build.filelist_type {
            FilelistType::Absolute => format!("{}\n", path.to_string_lossy()),
            FilelistType::Relative => {
                let relative = path.strip_prefix(&base_path).into_diagnostic()?;
                format!("{}\n", relative.to_string_lossy())
            }
            FilelistType::Flgen => {
                let relative = path.strip_prefix(&base_path).into_diagnostic()?;
                format!("source_file '{}'\n", relative.to_string_lossy())
            }
        })
    }

    fn gen_filelist(
        &self,
        metadata: &mut Metadata,
        paths: &[PathSet],
        temp_dir: Option<TempDir>,
        include_tests: bool,
    ) -> Result<()> {
        let filelist_path = metadata.filelist_path();
        let base_path = metadata.output_dir();

        let paths = Self::sort_filelist(metadata, paths, include_tests);

        let text = if let Target::Bundle { path } = &metadata.build.target {
            let temp_dir = temp_dir.unwrap();
            let mut text = String::new();
            let target_path = base_path.join(path);

            for path in paths {
                let dst = temp_dir
                    .path()
                    .join(path.dst.strip_prefix(&base_path).into_diagnostic()?);

                text.push_str(&fs::read_to_string(&dst).into_diagnostic()?);
            }

            if let Some(parent) = target_path.parent()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent).into_diagnostic()?;
            }
            let written = utils::write_file_if_changed(&target_path, text.as_bytes())?;
            if written {
                debug!("Output file ({})", target_path.to_string_lossy());
            }

            metadata.add_generated_file(target_path.clone());

            self.gen_filelist_line(metadata, &target_path)?
        } else {
            let mut text = String::new();
            for path in paths {
                let line = self.gen_filelist_line(metadata, &path.dst)?;
                text.push_str(&line);
            }
            text
        };

        if let Some(parent) = filelist_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).into_diagnostic()?;
        }
        utils::write_file_if_changed(&filelist_path, text.as_bytes())?;

        info!("Output filelist ({})", filelist_path.to_string_lossy());
        metadata.add_generated_file(filelist_path);

        Ok(())
    }

    pub fn sort_filelist(
        metadata: &Metadata,
        paths: &[PathSet],
        include_tests: bool,
    ) -> Vec<PathSet> {
        let mut table = HashMap::new();
        for path in paths {
            table.insert(path.src.clone(), path);
        }

        // Collect files connected from project
        let mut prj_namespace = Namespace::new();
        prj_namespace.push(resource_table::insert_str(&metadata.project.name));

        let mut candidate_symbols: Vec<_> = type_dag::connected_components()
            .into_iter()
            .filter(|symbols| symbols[0].namespace.included(&prj_namespace))
            .flatten()
            .collect();
        if include_tests {
            candidate_symbols.extend(symbol_table::get_all().into_iter().filter(|symbol| {
                matches!(symbol.kind, SymbolKind::Test(_))
                    && symbol.namespace.included(&prj_namespace)
            }));
        }

        let mut used_paths = HashMap::new();
        for symbol in &candidate_symbols {
            if let TokenSource::File { path, .. } = symbol.token.source {
                let path = PathBuf::from(format!("{path}"));
                if let Some(x) = table.remove(&path) {
                    used_paths.insert(path, x);
                }
            }
        }

        let mut ret = vec![];
        let sorted_symbols = type_dag::toposort();
        for symbol in sorted_symbols {
            if matches!(
                symbol.kind,
                SymbolKind::Module(_) | SymbolKind::Interface(_) | SymbolKind::Package(_)
            ) && let TokenSource::File { path, .. } = symbol.token.source
            {
                let path = PathBuf::from(format!("{path}"));
                if let Some(x) = used_paths.remove(&path) {
                    ret.push(x.clone());
                }
            }
        }

        let mut remaining: Vec<_> = used_paths.into_values().collect();
        remaining.sort_by(|a, b| a.src.cmp(&b.src));
        for path in remaining {
            ret.push(path.clone());
        }

        ret
    }

    pub fn check_skip(metadata: &Metadata, contexts: &mut Vec<Context>) {
        let mut updated_files = HashSet::new();
        let dependent_files = veryl_analyzer::type_dag::dependent_files();
        for context in contexts.iter() {
            let updated = if let Some(generated) =
                metadata.build_info.generated_files.get(&context.path.dst)
            {
                context.modified > *generated
            } else {
                true
            };
            if updated {
                let path = resource_table::insert_path(&context.path.src);
                updated_files.insert(path);

                if let Some(dependents) = dependent_files.get(&path) {
                    for x in dependents {
                        updated_files.insert(*x);
                    }
                }
            }
        }
        for context in contexts {
            let path = resource_table::insert_path(&context.path.src);
            if !updated_files.contains(&path) {
                context.skip = true;
                debug!(
                    "Skipping unmodified file ({})",
                    context.path.src.to_string_lossy()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static BUILD_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct CurrentDirGuard {
        path: PathBuf,
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.path).unwrap();
        }
    }

    fn set_current_dir(path: &Path) -> CurrentDirGuard {
        let current = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        CurrentDirGuard { path: current }
    }

    fn create_project(root: &Path, name: &str, filelist_type: FilelistType) -> (Metadata, PathBuf) {
        let project_path = root.join(name);
        let src_path = project_path.join("src");
        fs::create_dir_all(&src_path).unwrap();

        let filelist_type = match filelist_type {
            FilelistType::Absolute => "absolute",
            FilelistType::Relative => "relative",
            FilelistType::Flgen => "flgen",
        };
        fs::write(
            project_path.join("Veryl.toml"),
            format!(
                r#"[project]
name = "{name}"
version = "0.1.0"

[build]
sources = ["src"]
target = {{type = "directory", path = "target"}}
filelist_type = "{filelist_type}"
sourcemap_target = {{type = "none"}}
exclude_std = true
"#
            ),
        )
        .unwrap();
        fs::write(src_path.join("foo.veryl"), "module Foo {}\n").unwrap();

        let metadata = Metadata::load(project_path.join("Veryl.toml")).unwrap();
        (metadata, project_path)
    }

    fn run_build(metadata: &mut Metadata, out_dir: Option<PathBuf>) {
        Analyzer::new(metadata).clear();

        let build = CmdBuild::new(OptBuild {
            files: Vec::new(),
            check: false,
            out_dir,
        });
        build
            .exec(metadata, false, true, None, None, &[])
            .expect("build should succeed");

        Analyzer::new(metadata).clear();
    }

    #[test]
    fn build_without_out_dir_uses_project_output_dir() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_project(tempdir.path(), "default_out", FilelistType::Absolute);

        run_build(&mut metadata, None);

        assert!(project_path.join("target/foo.sv").exists());
        assert!(project_path.join("default_out.f").exists());
        assert!(project_path.join("dependencies").is_dir());
    }

    #[test]
    fn build_with_absolute_out_dir_moves_generated_outputs() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_project(tempdir.path(), "absolute_out", FilelistType::Absolute);
        let out_dir = tempdir.path().join("outside-project");

        run_build(&mut metadata, Some(out_dir.clone()));

        let out_dir = out_dir.canonicalize().unwrap();
        let generated = out_dir.join("target/foo.sv");
        let filelist = out_dir.join("absolute_out.f");
        assert!(generated.exists());
        assert!(filelist.exists());
        assert!(out_dir.join("dependencies").is_dir());
        assert!(!project_path.join("target/foo.sv").exists());
        assert!(!project_path.join("absolute_out.f").exists());
        assert!(!project_path.join("dependencies").exists());

        let filelist_text = fs::read_to_string(filelist).unwrap();
        assert!(
            filelist_text
                .lines()
                .any(|line| line == generated.to_string_lossy())
        );
    }

    #[test]
    fn build_with_relative_out_dir_resolves_from_current_dir() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_project(tempdir.path(), "relative_out", FilelistType::Relative);
        let _guard = set_current_dir(&project_path);

        run_build(&mut metadata, Some(PathBuf::from("build-output")));

        let out_dir = project_path.join("build-output");
        let filelist = out_dir.join("relative_out.f");
        assert!(out_dir.join("target/foo.sv").exists());
        assert!(filelist.exists());
        assert!(out_dir.join("dependencies").is_dir());

        let filelist_text = fs::read_to_string(filelist).unwrap();
        let relative_path = Path::new("target").join("foo.sv");
        assert!(
            filelist_text
                .lines()
                .any(|line| line == relative_path.to_string_lossy())
        );
    }

    const INC_FILE_A: &str = r#"
    package PackageA {
        const WIDTH: u32 = 8;
    }
    "#;

    const INC_FILE_B: &str = r#"
    module ModuleB (
        o_dat: output logic<PackageA::WIDTH>,
    ) {
        assign o_dat = 0;
    }
    "#;

    const INC_FILE_B2: &str = r#"
    module ModuleB (
        o_dat: output logic<PackageA::WIDTH>,
    ) {
        assign o_dat = 1;
    }

    module ModuleB2 (
        o_dat: output logic<PackageA::WIDTH>,
    ) {
        inst u0: ModuleB (o_dat);
    }
    "#;

    fn create_incremental_project(root: &Path, name: &str) -> (Metadata, PathBuf) {
        let project_path = root.join(name);
        let src_path = project_path.join("src");
        fs::create_dir_all(&src_path).unwrap();

        fs::write(
            project_path.join("Veryl.toml"),
            format!(
                r#"[project]
name = "{name}"
version = "0.1.0"

[build]
sources = ["src"]
target = {{type = "directory", path = "target"}}
sourcemap_target = {{type = "none"}}
exclude_std = true
incremental = true
"#
            ),
        )
        .unwrap();
        fs::write(src_path.join("a.veryl"), INC_FILE_A).unwrap();
        fs::write(src_path.join("b.veryl"), INC_FILE_B).unwrap();

        let mut metadata = Metadata::load(project_path.join("Veryl.toml")).unwrap();
        // What main.rs records after a build; required for any restore.
        metadata.build_info.veryl_version = Some(veryl_metadata::VERYL_VERSION.to_string());
        (metadata, project_path)
    }

    #[test]
    fn incremental_build_restores_fragments_and_matches_cold_output() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_incremental_project(tempdir.path(), "incremental");

        // Cold build: populates the cache, restores nothing.
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 0);
        assert!(project_path.join(".build/cache/manifest.toml").exists());
        let cold_a = fs::read_to_string(project_path.join("target/a.sv")).unwrap();
        let cold_b = fs::read_to_string(project_path.join("target/b.sv")).unwrap();

        // Warm build with no changes: everything is restored, outputs stay.
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 2);
        assert_eq!(
            fs::read_to_string(project_path.join("target/a.sv")).unwrap(),
            cold_a
        );
        assert_eq!(
            fs::read_to_string(project_path.join("target/b.sv")).unwrap(),
            cold_b
        );

        // Change b.veryl: a.veryl is restored, and b's pass2/emit resolve
        // PackageA through the restored symbols.
        fs::write(project_path.join("src/b.veryl"), INC_FILE_B2).unwrap();
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 1);
        let warm_b = fs::read_to_string(project_path.join("target/b.sv")).unwrap();
        assert!(warm_b.contains("ModuleB2"));
        assert_eq!(
            fs::read_to_string(project_path.join("target/a.sv")).unwrap(),
            cold_a
        );

        // The warm output must match a from-scratch build of the same
        // sources (same project name in a separate root, because the
        // project name prefixes emitted module names).
        let cold_tempdir = tempfile::tempdir().unwrap();
        let (mut cold_metadata, cold_path) =
            create_incremental_project(cold_tempdir.path(), "incremental");
        fs::write(cold_path.join("src/b.veryl"), INC_FILE_B2).unwrap();
        run_build(&mut cold_metadata, None);
        assert_eq!(
            fs::read_to_string(cold_path.join("target/b.sv")).unwrap(),
            warm_b
        );
    }

    #[test]
    fn incremental_build_change_in_dependency_rebuilds_dependents() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_incremental_project(tempdir.path(), "incremental_dep");

        run_build(&mut metadata, None);

        // Changing the package invalidates its dependent b.veryl too.
        fs::write(
            project_path.join("src/a.veryl"),
            INC_FILE_A.replace("8", "16"),
        )
        .unwrap();
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 0);
        let b = fs::read_to_string(project_path.join("target/b.sv")).unwrap();
        assert!(b.contains("WIDTH"));
    }

    #[test]
    fn incremental_build_corrupt_cache_falls_back() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_incremental_project(tempdir.path(), "incremental_corrupt");

        run_build(&mut metadata, None);
        let cold_b = fs::read_to_string(project_path.join("target/b.sv")).unwrap();

        // Truncate every fragment blob; restore must fall back cleanly.
        let fragments = project_path.join(".build/cache/fragments");
        for dir in fs::read_dir(&fragments).unwrap().flatten() {
            for file in fs::read_dir(dir.path()).unwrap().flatten() {
                fs::write(file.path(), b"garbage").unwrap();
            }
        }
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 0);
        assert_eq!(
            fs::read_to_string(project_path.join("target/b.sv")).unwrap(),
            cold_b
        );
    }
}
