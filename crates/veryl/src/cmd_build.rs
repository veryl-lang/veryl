use crate::cmd_check::CheckError;
use crate::OptBuild;
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;
use veryl_analyzer::Analyzer;
use veryl_emitter::Emitter;
use veryl_metadata::{FilelistType, Metadata, PathPair};
use veryl_parser::Parser;

pub struct CmdBuild {
    opt: OptBuild,
}

impl CmdBuild {
    pub fn new(opt: OptBuild) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let now = Instant::now();

        let paths = metadata.paths(&self.opt.files, false)?;

        let mut check_error = CheckError::default();
        let mut contexts = Vec::new();

        for path in &paths {
            self.print(&format!(
                "[Info] Processing file: {}",
                path.src.to_string_lossy()
            ));

            let input = fs::read_to_string(&path.src)
                .into_diagnostic()
                .wrap_err("")?;
            let parser = Parser::parse(&input, &path.src)?;

            let mut analyzer = Analyzer::new(&input, &path.prj);
            let errors = analyzer.analyze_tree(&parser.veryl);
            for error in errors {
                check_error.related.push(error);
            }

            contexts.push((path, input, parser));
        }

        for (path, input, _) in &contexts {
            let errors = Analyzer::analyze_post(&path.src, input);
            if !errors.is_empty() {
                for error in errors {
                    check_error.related.push(error);
                }
                return Err(check_error.into());
            }
        }

        if !check_error.related.is_empty() {
            return Err(check_error.into());
        }

        for (path, _, parser) in &contexts {
            let mut emitter = Emitter::new(metadata);
            emitter.emit(&parser.veryl);

            self.print(&format!(
                "[Info] Output file: {}",
                path.dst.to_string_lossy()
            ));

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
        }

        self.gen_filelist(metadata, &paths)?;

        let elapsed_time = now.elapsed();
        self.print(&format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ));

        Ok(true)
    }

    fn gen_filelist(&self, metadata: &Metadata, paths: &[PathPair]) -> Result<()> {
        let filelist_name = match metadata.build.filelist_type {
            FilelistType::Absolute => format!("{}.f", metadata.project.name),
            FilelistType::Relative => format!("{}.f", metadata.project.name),
            FilelistType::Flgen => format!("{}.list.rb", metadata.project.name),
        };

        let filelist_path = metadata.metadata_path.with_file_name(filelist_name);
        let base_path = metadata.metadata_path.parent().unwrap();

        let mut text = String::new();
        for path in paths {
            let path = path.dst.canonicalize().into_diagnostic()?;
            let relative = path.strip_prefix(base_path).into_diagnostic()?;
            let line = match metadata.build.filelist_type {
                FilelistType::Absolute => format!("{}\n", path.to_string_lossy()),
                FilelistType::Relative => format!("{}\n", relative.to_string_lossy()),
                FilelistType::Flgen => format!("source_file '{}'\n", relative.to_string_lossy()),
            };
            text.push_str(&line);
        }

        self.print(&format!(
            "[Info] Output filelist: {}",
            filelist_path.to_string_lossy()
        ));
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

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
    }
}
