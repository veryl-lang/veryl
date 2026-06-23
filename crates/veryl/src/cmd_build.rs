use crate::OptBuild;
use crate::StopWatch;
use crate::diff::print_diff;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput};
use crate::utils;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::{symbol_table, type_dag};
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, SourceMapTarget, Target};
use veryl_parser::{resource_table, veryl_token::TokenSource};
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
        ir: Option<&mut veryl_analyzer::ir::Ir>,
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

        let options = AnalyzeOptions {
            defines,
            emit_mode: true,
            incremental: true,
            fail_fast: true,
        };
        let AnalyzeOutput {
            mut contexts,
            incremental,
            check_error,
            filelist_excluded,
        } = pipeline::analyze(metadata, &paths, options, ir, test_filter)?;

        let mut stopwatch = StopWatch::new();

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
                emitter.emit(&context.parser.veryl, &context.input);

                let dst_dir = dst.parent().unwrap();
                if !dst_dir.exists() {
                    std::fs::create_dir_all(dst.parent().unwrap()).into_diagnostic()?;
                }

                let exclude_check = context.path.prj == "$std";
                let bundle = temp_dir.is_some();

                if self.opt.check && bundle {
                    // Stage in the temp dir; the bundle is compared as a whole
                    // after the loop.
                    utils::write_file_if_changed(&dst, emitter.as_str().as_bytes())?;
                } else if self.opt.check && !exclude_check {
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
            self.gen_filelist(
                metadata,
                &paths,
                temp_dir,
                include_tests,
                &filelist_excluded,
            )?;
        } else if let Some(temp_dir) = &temp_dir
            && !self.check_bundle(
                metadata,
                &paths,
                temp_dir,
                include_tests,
                &filelist_excluded,
                quiet,
            )?
        {
            all_pass = false;
        }

        debug!("Executed filelist ({} milliseconds)", stopwatch.lap());

        if let Some(mut inc) = incremental {
            inc.save(&pipeline::collect_diagnosed(&check_error));
            debug!("Saved fragment cache ({} milliseconds)", stopwatch.lap());
        }

        // No-op (analyze already returned Ok), kept for symmetry with check.
        let _ = check_error.check_err()?;

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
        excluded: &HashSet<PathBuf>,
    ) -> Result<()> {
        let filelist_path = metadata.filelist_path();
        let base_path = metadata.output_dir();

        let mut paths = Self::sort_filelist(metadata, paths, include_tests);
        // Drop entries that were intentionally not emitted (e.g. testbench
        // files filtered out by `--test`); their .sv may not exist on disk.
        paths.retain(|path| !excluded.contains(&path.src));

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

    /// Check-mode counterpart of the bundle branch in [`Self::gen_filelist`]:
    /// a bundle target's per-file outputs never reach disk, so compare the
    /// assembled bundle rather than each file.
    fn check_bundle(
        &self,
        metadata: &Metadata,
        paths: &[PathSet],
        temp_dir: &TempDir,
        include_tests: bool,
        excluded: &HashSet<PathBuf>,
        quiet: bool,
    ) -> Result<bool> {
        let Target::Bundle { path } = &metadata.build.target else {
            return Ok(true);
        };
        let base_path = metadata.output_dir();
        let target_path = base_path.join(path);

        let mut paths = Self::sort_filelist(metadata, paths, include_tests);
        paths.retain(|path| !excluded.contains(&path.src));

        let mut text = String::new();
        for path in &paths {
            let dst = temp_dir
                .path()
                .join(path.dst.strip_prefix(&base_path).into_diagnostic()?);
            text.push_str(&fs::read_to_string(&dst).into_diagnostic()?);
        }

        let output = fs::read_to_string(&target_path).unwrap_or_default();
        if output == text {
            Ok(true)
        } else {
            if !quiet {
                print_diff(&target_path, &output, &text);
            }
            Ok(false)
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use veryl_analyzer::Analyzer;

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

    fn write_incremental_project(
        root: &Path,
        name: &str,
        files: &[(&str, &str)],
    ) -> (Metadata, PathBuf) {
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
        for (file_name, content) in files {
            fs::write(src_path.join(file_name), content).unwrap();
        }

        // The cache is keyed on the binary, and the in-memory metadata carries
        // generated_files across runs — so these tests need no info.toml.
        let metadata = Metadata::load(project_path.join("Veryl.toml")).unwrap();
        (metadata, project_path)
    }

    fn create_incremental_project(root: &Path, name: &str) -> (Metadata, PathBuf) {
        write_incremental_project(
            root,
            name,
            &[("a.veryl", INC_FILE_A), ("b.veryl", INC_FILE_B)],
        )
    }

    const CLEAN_MODULE: &str = r#"
    module Clean (
        o_dat: output logic,
    ) {
        assign o_dat = 0;
    }
    "#;

    // Pass1-clean, but warns (`unused_var`) in post-pass2. That pass runs every
    // build over the restored symbol table, so a warm run re-derives the warning
    // and also replays the cached copy — the dedup keeps it to one.
    const WARNING_MODULE: &str = r#"
    module Warn {
        let unused_var: logic = 1;
    }
    "#;

    const ERROR_MODULE: &str = r#"
    module Bad {
        let broken: logic = undefined_signal;
    }
    "#;

    // Distinct module name from CLEAN_MODULE to avoid a duplicate definition.
    const FIXED_MODULE: &str = r#"
    module Fixed (
        o_dat: output logic,
    ) {
        assign o_dat = 0;
    }
    "#;

    fn run_check(metadata: &mut Metadata) -> Result<bool> {
        Analyzer::new(metadata).clear();

        let check = crate::cmd_check::CmdCheck::new(crate::OptCheck { files: Vec::new() });
        let ret = check.exec(metadata);

        Analyzer::new(metadata).clear();
        ret
    }

    const BUNDLE_TOP: &str = r#"
    module Top (
        o_dat: output logic,
    ) {
        inst u: Sub (o_dat);
    }
    "#;

    const BUNDLE_SUB: &str = r#"
    module Sub (
        o_dat: output logic,
    ) {
        assign o_dat = 0;
    }
    "#;

    fn create_bundle_project(root: &Path, name: &str) -> (Metadata, PathBuf) {
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
target = {{type = "bundle", path = "bundled.sv"}}
sourcemap_target = {{type = "none"}}
exclude_std = true
"#
            ),
        )
        .unwrap();
        fs::write(src_path.join("top.veryl"), BUNDLE_TOP).unwrap();
        fs::write(src_path.join("sub.veryl"), BUNDLE_SUB).unwrap();

        let metadata = Metadata::load(project_path.join("Veryl.toml")).unwrap();
        (metadata, project_path)
    }

    fn run_build_check(metadata: &mut Metadata) -> bool {
        Analyzer::new(metadata).clear();

        let build = CmdBuild::new(OptBuild {
            files: Vec::new(),
            check: true,
            out_dir: None,
        });
        let pass = build
            .exec(metadata, false, true, None, None, &[])
            .expect("check should run");

        Analyzer::new(metadata).clear();
        pass
    }

    // Regression: check mode compared each file against an empty temp path, so
    // `build --check` never passed on a freshly built bundle.
    #[test]
    fn bundle_check_passes_after_build() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) = create_bundle_project(tempdir.path(), "bundle_check");

        run_build(&mut metadata, None);
        assert!(project_path.join("bundled.sv").exists());

        assert!(
            run_build_check(&mut metadata),
            "check must pass on the just-built bundle"
        );
    }

    #[test]
    fn bundle_check_detects_change() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_bundle_project(tempdir.path(), "bundle_check_change");

        run_build(&mut metadata, None);

        // Edit a source so the regenerated bundle differs.
        fs::write(
            project_path.join("src/sub.veryl"),
            r#"
    module Sub (
        o_dat: output logic,
    ) {
        assign o_dat = 1;
    }
    "#,
        )
        .unwrap();

        assert!(
            !run_build_check(&mut metadata),
            "check must detect the changed bundle"
        );
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

    const INC_FILE_T: &str = r#"
    #[test(test_modb)]
    module test_modb {
        var d: logic<PackageA::WIDTH>;

        inst u0: ModuleB (
            o_dat: d,
        );
    }
    "#;

    fn run_build_with_ir(metadata: &mut Metadata) -> veryl_analyzer::ir::Ir {
        Analyzer::new(metadata).clear();

        let build = CmdBuild::new(OptBuild {
            files: Vec::new(),
            check: false,
            out_dir: None,
        });
        let mut ir = veryl_analyzer::ir::Ir::default();
        build
            .exec(metadata, true, true, Some(&mut ir), None, &[])
            .expect("build should succeed");

        Analyzer::new(metadata).clear();
        ir
    }

    fn ir_module_names(ir: &veryl_analyzer::ir::Ir) -> Vec<String> {
        ir.components
            .iter()
            .filter_map(|x| match x {
                veryl_analyzer::ir::Component::Module(m) => Some(m.name.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn incremental_test_build_keeps_test_ir() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_incremental_project(tempdir.path(), "incremental_test");
        fs::write(project_path.join("src/t.veryl"), INC_FILE_T).unwrap();

        // Cold build with a caller-supplied IR (the `veryl test` path).
        let ir = run_build_with_ir(&mut metadata);
        assert!(ir_module_names(&ir).iter().any(|x| x == "test_modb"));

        // Warm build: a.veryl/b.veryl are restored, but the test file must
        // be re-analyzed so the simulated top stays in the IR (it
        // elaborates its instance tree from the restored definitions).
        let ir = run_build_with_ir(&mut metadata);
        assert_eq!(crate::incremental::last_restored_count(), 2);
        assert!(ir_module_names(&ir).iter().any(|x| x == "test_modb"));
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

    #[test]
    fn incremental_check_restores_on_warm_run() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, _project_path) =
            create_incremental_project(tempdir.path(), "inc_check_warm");

        run_check(&mut metadata).expect("clean check passes");
        assert_eq!(crate::incremental::last_restored_count(), 0);

        // Warm: both clean files restore even though check emits nothing.
        run_check(&mut metadata).expect("clean check passes");
        assert_eq!(crate::incremental::last_restored_count(), 2);
    }

    #[test]
    fn build_and_check_share_cache() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, _project_path) =
            create_incremental_project(tempdir.path(), "build_check_share");

        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 0);

        // Check reuses the build's clean fragments (consider_output is false,
        // so output freshness never blocks the hit).
        run_check(&mut metadata).expect("clean check passes");
        assert!(crate::incremental::last_restored_count() > 0);
    }

    #[test]
    fn incremental_check_restores_warning_file_and_re_reports() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, _project_path) = write_incremental_project(
            tempdir.path(),
            "inc_check_warn",
            &[
                ("clean.veryl", CLEAN_MODULE),
                ("warn.veryl", WARNING_MODULE),
            ],
        );

        // Cold check fails on the warning; both files are cached.
        assert!(run_check(&mut metadata).is_err());

        // Warm: both files restore, yet check still fails — the warning is
        // re-reported (post-pass2 re-derives it; the cached copy is deduped).
        assert!(run_check(&mut metadata).is_err());
        assert_eq!(crate::incremental::last_restored_count(), 2);
    }

    /// One analyze pass through the same pipeline `veryl check` uses, persisting
    /// the cache for the next call. Returns (diagnostic count, restored count).
    fn analyze_diagnostic_count(metadata: &mut Metadata) -> (usize, usize) {
        Analyzer::new(metadata).clear();
        let paths = metadata
            .paths(&Vec::<PathBuf>::new(), true, true)
            .expect("paths");
        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: true,
            fail_fast: true,
        };
        let AnalyzeOutput {
            incremental,
            check_error,
            ..
        } = pipeline::analyze(metadata, &paths, options, None, None).expect("analyze");
        let restored = incremental.as_ref().map_or(0, |x| x.restored);
        let count = check_error.related.len();
        if let Some(mut inc) = incremental {
            inc.save(&pipeline::collect_diagnosed(&check_error));
        }
        Analyzer::new(metadata).clear();
        (count, restored)
    }

    #[test]
    fn incremental_warm_run_reports_each_warning_once() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, _project_path) = write_incremental_project(
            tempdir.path(),
            "inc_warn_once",
            &[
                ("clean.veryl", CLEAN_MODULE),
                ("warn.veryl", WARNING_MODULE),
            ],
        );

        // Cold: the single `unused_var` warning is produced once.
        let (cold, cold_restored) = analyze_diagnostic_count(&mut metadata);
        assert_eq!(cold_restored, 0);
        assert_eq!(cold, 1);

        // Warm: both files restore. post-pass2 re-derives the warning from the
        // restored symbol table while the cache also replays it; the dedup must
        // collapse them so the warning is reported exactly once, not twice.
        let (warm, warm_restored) = analyze_diagnostic_count(&mut metadata);
        assert_eq!(warm_restored, 2);
        assert_eq!(warm, cold);
    }

    #[test]
    fn incremental_build_restores_warning_file() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, _project_path) = write_incremental_project(
            tempdir.path(),
            "inc_build_warn",
            &[
                ("clean.veryl", CLEAN_MODULE),
                ("warn.veryl", WARNING_MODULE),
            ],
        );

        // Cold build caches both files (warnings don't fail a build).
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 0);

        // Warm build restores both, including the warning file.
        run_build(&mut metadata, None);
        assert_eq!(crate::incremental::last_restored_count(), 2);
    }

    #[test]
    fn incremental_check_error_file_not_cached() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) = write_incremental_project(
            tempdir.path(),
            "inc_check_err",
            &[("clean.veryl", CLEAN_MODULE), ("bad.veryl", ERROR_MODULE)],
        );

        // A fatal error aborts before save; must error without panicking.
        assert!(run_check(&mut metadata).is_err());

        // Fixing it passes: no stale error state lingers from the aborted run.
        fs::write(project_path.join("src/bad.veryl"), FIXED_MODULE).unwrap();
        run_check(&mut metadata).expect("check passes after the fix");
    }

    const TF_DUT: &str = r#"
    module Dut (
        o: output logic,
    ) {
        assign o = 0;
    }
    "#;

    const TF_TEST_A: &str = r#"
    #[test(test_a)]
    module test_a {
        initial {
            $display("a");
        }
    }
    "#;

    const TF_TEST_B: &str = r#"
    #[test(test_b)]
    module test_b {
        initial {
            $display("b");
        }
    }
    "#;

    // Regression for #2812: `veryl test --test <filter>` on a clean tree marks
    // non-matching testbench files skip (never emitted), but the filelist
    // generator used to canonicalize their dst .sv -> ENOENT.
    #[test]
    fn test_filter_on_clean_tree_excludes_unemitted_tests_from_filelist() {
        let _lock = BUILD_TEST_LOCK.lock().unwrap();
        let tempdir = tempfile::tempdir().unwrap();
        let (mut metadata, project_path) =
            create_project(tempdir.path(), "test_filter", FilelistType::Absolute);
        let src = project_path.join("src");
        fs::write(src.join("dut.veryl"), TF_DUT).unwrap();
        fs::write(src.join("test_a.veryl"), TF_TEST_A).unwrap();
        fs::write(src.join("test_b.veryl"), TF_TEST_B).unwrap();

        // Emit into a fresh out_dir so no prior `.sv` exists (clean tree).
        let out_dir = tempdir.path().join("clean-out");

        Analyzer::new(&metadata).clear();
        let build = CmdBuild::new(OptBuild {
            files: Vec::new(),
            check: false,
            out_dir: Some(out_dir.clone()),
        });
        let mut ir = veryl_analyzer::ir::Ir::default();
        build
            .exec(
                &mut metadata,
                true,
                true,
                Some(&mut ir),
                Some("test_a"),
                &[],
            )
            .expect("filtered test build on a clean tree should succeed");
        Analyzer::new(&metadata).clear();

        let out_dir = out_dir.canonicalize().unwrap();
        let filelist = fs::read_to_string(out_dir.join("test_filter.f")).unwrap();
        assert!(
            filelist.contains("test_a.sv"),
            "matching testbench must be listed: {filelist}"
        );
        assert!(
            filelist.contains("dut.sv"),
            "DUT must be listed: {filelist}"
        );
        assert!(
            !filelist.contains("test_b.sv"),
            "filtered-out testbench (never emitted) must not be listed: {filelist}"
        );
    }
}
