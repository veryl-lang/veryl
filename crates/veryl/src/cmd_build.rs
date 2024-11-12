use crate::cmd_check::CheckError;
use crate::OptBuild;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::{symbol_table, type_dag, Analyzer};
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, SourceMapTarget, Target};
use veryl_parser::{resource_table, veryl_token::TokenSource, Parser};
use veryl_path::PathSet;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata, include_tests: bool) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true)?;

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            info!("Processing file ({})", path.src.to_string_lossy());

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let analyzer = Analyzer::new(metadata);
            let mut errors = analyzer.analyze_pass1(&path.prj, &input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;

            contexts.push((path, input, parser, analyzer));
        }

        Analyzer::analyze_post_pass1();

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass2(&path.prj, input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass3(&path.prj, input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        let temp_dir = if let Target::Bundle { .. } = &metadata.build.target {
            Some(TempDir::new().into_diagnostic()?)
        } else {
            None
        };

        for (path, input, parser, _) in &contexts {
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
            emitter.emit(&path.prj, &parser.veryl);

            let dst_dir = dst.parent().unwrap();
            if !dst_dir.exists() {
                std::fs::create_dir_all(dst.parent().unwrap()).into_diagnostic()?;
            }

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&dst)
                .into_diagnostic()?;
            file.write_all(emitter.as_str().as_bytes())
                .into_diagnostic()?;
            file.flush().into_diagnostic()?;

            debug!("Output file ({})", dst.to_string_lossy());

            if metadata.build.sourcemap_target != SourceMapTarget::None {
                let source_map = emitter.source_map();
                source_map.set_source_content(input);
                let source_map = source_map.to_bytes().into_diagnostic()?;

                let map_dir = map.parent().unwrap();
                if !map_dir.exists() {
                    std::fs::create_dir_all(map.parent().unwrap()).into_diagnostic()?;
                }

                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&map)
                    .into_diagnostic()?;
                file.write_all(&source_map).into_diagnostic()?;
                file.flush().into_diagnostic()?;

                debug!("Output map ({})", map.to_string_lossy());
            }
        }

        self.gen_filelist(metadata, &paths, temp_dir, include_tests)?;

        let _ = check_error.check_all()?;
        Ok(true)
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
        metadata: &Metadata,
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

            debug!("Output file ({})", target_path.to_string_lossy());
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&target_path)
                .into_diagnostic()?;
            file.write_all(text.as_bytes()).into_diagnostic()?;
            file.flush().into_diagnostic()?;

            self.gen_filelist_line(metadata, &target_path)?
        } else {
            let mut text = String::new();
            for path in paths {
                let line = self.gen_filelist_line(metadata, &path.dst)?;
                text.push_str(&line);
            }
            text
        };

        info!("Output filelist ({})", filelist_path.to_string_lossy());
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(filelist_path)
            .into_diagnostic()?;
        file.write_all(text.as_bytes()).into_diagnostic()?;
        file.flush().into_diagnostic()?;

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
            if let TokenSource::File(x) = symbol.token.source {
                let path = PathBuf::from(format!("{}", x));
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
            ) {
                if let TokenSource::File(x) = symbol.token.source {
                    let path = PathBuf::from(format!("{}", x));
                    if let Some(x) = used_paths.remove(&path) {
                        ret.push(x.clone());
                    }
                }
            }
        }

        for path in used_paths.into_values() {
            ret.push(path.clone());
        }

        ret
    }
}
