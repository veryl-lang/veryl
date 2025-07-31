use miette::{IntoDiagnostic, Result};
use std::fs;
use std::time::SystemTime;
use veryl_analyzer::Analyzer;
use veryl_parser::Parser;
use veryl_path::PathSet;

pub struct Context {
    pub path: PathSet,
    pub input: String,
    pub parser: Parser,
    pub analyzer: Analyzer,
    pub modified: SystemTime,
    pub skip: bool,
}

impl Context {
    pub fn new(path: PathSet, input: String, parser: Parser, analyzer: Analyzer) -> Result<Self> {
        let file_metadata = fs::metadata(&path.src).into_diagnostic()?;
        let modified = file_metadata.modified().into_diagnostic()?;
        Ok(Self {
            path,
            input,
            parser,
            analyzer,
            modified,
            skip: false,
        })
    }
}
