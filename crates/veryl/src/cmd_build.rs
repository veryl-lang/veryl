use crate::OptBuild;
use crate::StopWatch;
use crate::cmd_check::CheckError;
use crate::context::Context;
use crate::diff::print_diff;
use crate::utils;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::TempDir;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::{Analyzer, symbol_table, type_dag};
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, SourceMapTarget, Target};
use veryl_parser::{Parser, resource_table, veryl_token::TokenSource};
use veryl_path::PathSet;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata, include_tests: bool, quiet: bool) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        let mut check_error = CheckError::new(metadata.build.error_count_limit);
        let mut contexts = Vec::new();

        let mut stopwatch = StopWatch::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let analyzer = Analyzer::new(metadata);
            let mut errors = analyzer.analyze_pass1(&path.prj, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;

            let context = Context::new(path.clone(), input, parser, analyzer)?;
            contexts.push(context);
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

        for context in &contexts {
            if !context.skip {
                let path = &context.path;
                let mut errors =
                    context
                        .analyzer
                        .analyze_pass2(&path.prj, &path.src, &context.parser.veryl);
                check_error = check_error.append(&mut errors).check_err()?;
            }
        }

        debug!("Executed analyze_pass2 ({} milliseconds)", stopwatch.lap());

        let info = Analyzer::analyze_post_pass2();

        for context in &contexts {
            if !context.skip {
                let path = &context.path;
                let mut errors = context.analyzer.analyze_pass3(
                    &path.prj,
                    &path.src,
                    &context.parser.veryl,
                    &info,
                );
                check_error = check_error.append(&mut errors).check_err()?;
            }
        }

        debug!("Executed analyze_pass3 ({} milliseconds)", stopwatch.lap());

        let temp_dir = if let Target::Bundle { .. } = &metadata.build.target {
            Some(TempDir::new().into_diagnostic()?)
        } else {
            None
        };

        let mut all_pass = true;
        for context in &contexts {
            if !context.skip {
                let path = &context.path;
                let (dst, map) = if let Some(ref temp_dir) = temp_dir {
                    let dst_temp = temp_dir.path().join(
                        path.dst
                            .strip_prefix(metadata.project_path())
                            .into_diagnostic()?,
                    );
                    let map_temp = temp_dir.path().join(
                        path.map
                            .strip_prefix(metadata.project_path())
                            .into_diagnostic()?,
                    );
                    (dst_temp, map_temp)
                } else {
                    (path.dst.clone(), path.map.clone())
                };

                let mut emitter = Emitter::new(metadata, &path.src, &dst, &map);
                emitter.emit(&path.prj, &context.parser.veryl);

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

                    let now = SystemTime::now();
                    metadata.build_info.generated_files.insert(dst, now);

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

                        let now = SystemTime::now();
                        metadata.build_info.generated_files.insert(map, now);
                    }
                }
            }
        }

        if !self.opt.check {
            self.gen_filelist(metadata, &paths, temp_dir, include_tests)?;
        }

        let _ = check_error.check_err()?;
        Ok(all_pass)
    }

    fn gen_filelist_line(&self, metadata: &Metadata, path: &Path) -> Result<String> {
        let base_path = metadata.project_path();
        let path = path.canonicalize().into_diagnostic()?;
        let relative = path.strip_prefix(&base_path).into_diagnostic()?;
        Ok(match metadata.build.filelist_type {
            FilelistType::Absolute => format!("{}\n", path.to_string_lossy()),
            FilelistType::Relative => format!("{}\n", relative.to_string_lossy()),
            FilelistType::Flgen => {
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
        let base_path = metadata.project_path();

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

            let written = utils::write_file_if_changed(&target_path, text.as_bytes())?;
            if written {
                debug!("Output file ({})", target_path.to_string_lossy());
            }

            let now = SystemTime::now();
            metadata
                .build_info
                .generated_files
                .insert(target_path.clone(), now);

            self.gen_filelist_line(metadata, &target_path)?
        } else {
            let mut text = String::new();
            for path in paths {
                let line = self.gen_filelist_line(metadata, &path.dst)?;
                text.push_str(&line);
            }
            text
        };

        utils::write_file_if_changed(&filelist_path, text.as_bytes())?;

        info!("Output filelist ({})", filelist_path.to_string_lossy());
        let now = SystemTime::now();
        metadata
            .build_info
            .generated_files
            .insert(filelist_path, now);

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

        for path in used_paths.into_values() {
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
