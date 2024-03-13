use crate::cmd_check::CheckError;
use crate::OptBuild;
use log::{debug, info};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use veryl_analyzer::{type_dag, Analyzer};
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, PathPair};
use veryl_parser::{veryl_token::TokenSource, Parser};

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let now = Instant::now();

        let paths = metadata.paths(&self.opt.files)?;

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

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass2(&path.prj, input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        for (path, input, parser, analyzer) in &contexts {
            let mut errors = analyzer.analyze_pass3(&path.prj, input, &path.src, &parser.veryl);
            check_error = check_error.append(&mut errors).check_err()?;
        }

        for (path, _, parser, _) in &contexts {
            let mut emitter = Emitter::new(metadata);
            emitter.emit(&path.prj, &parser.veryl);

            let dst_dir = path.dst.parent().unwrap();
            if !dst_dir.exists() {
                std::fs::create_dir_all(path.dst.parent().unwrap()).into_diagnostic()?;
            }

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path.dst)
                .into_diagnostic()?;
            file.write_all(emitter.as_str().as_bytes())
                .into_diagnostic()?;
            file.flush().into_diagnostic()?;

            debug!("Output file ({})", path.dst.to_string_lossy());
        }

        self.gen_filelist(metadata, &paths)?;

        let elapsed_time = now.elapsed();
        debug!("Elapsed time ({} milliseconds)", elapsed_time.as_millis());

        let _ = check_error.check_all()?;
        Ok(true)
    }

    fn gen_filelist(&self, metadata: &Metadata, paths: &[PathPair]) -> Result<()> {
        let filelist_path = metadata.filelist_path();
        let base_path = metadata.project_path();

        let paths = Self::sort_filelist(paths);

        let mut text = String::new();
        for path in paths {
            let path = path.dst.canonicalize().into_diagnostic()?;
            let relative = path.strip_prefix(&base_path).into_diagnostic()?;
            let line = match metadata.build.filelist_type {
                FilelistType::Absolute => format!("{}\n", path.to_string_lossy()),
                FilelistType::Relative => format!("{}\n", relative.to_string_lossy()),
                FilelistType::Flgen => format!("source_file '{}'\n", relative.to_string_lossy()),
            };
            text.push_str(&line);
        }

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

    fn sort_filelist(paths: &[PathPair]) -> Vec<PathPair> {
        let mut table = HashMap::new();
        for path in paths {
            table.insert(path.src.clone(), path);
        }

        let mut ret = vec![];
        let sorted_tokens = type_dag::toposort();
        for token in sorted_tokens {
            if let TokenSource::File(x) = token.token.source {
                let path = PathBuf::from(format!("{}", x));
                if let Some(x) = table.remove(&path) {
                    ret.push(x.clone());
                }
            }
        }

        for path in table.into_values() {
            ret.push(path.clone());
        }

        ret
    }
}
