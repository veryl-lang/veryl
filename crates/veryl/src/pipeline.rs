//! Analysis pipeline (parse → pass1 → post_pass1 → pass2 → post_pass2) shared by
//! every command that analyzes Veryl: build/check/test/publish/doc/dump/synth.
//!
//! With `fail_fast` the first fatal error short-circuits to `Err`; otherwise an
//! `Ok` still carries any warnings in `check_error` for the caller's failure
//! policy. The fragment cache is keyed on a binary fingerprint plus the build
//! config, so a toolchain or config change discards it wholesale.

use crate::StopWatch;
use crate::context::Context;
use crate::incremental::Incremental;
use log::{debug, info};
use miette::{
    self, Diagnostic, IntoDiagnostic, LabeledSpan, Result, Severity, SourceCode, WrapErr,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;
use veryl_analyzer::{Analyzer, AnalyzerError, CachedDiagnostic};
use veryl_metadata::Metadata;
use veryl_parser::resource_table::PathId;
use veryl_parser::{Parser, resource_table};
use veryl_path::PathSet;

/// A diagnostic in [`CheckError`]: freshly produced this build (`Analyzer`),
/// or restored from the cache (`Cached`) and re-reported on a warm run.
#[derive(Debug)]
pub enum Diag {
    Analyzer(AnalyzerError),
    Cached(CachedDiagnostic),
}

impl Diag {
    fn is_error(&self) -> bool {
        match self {
            Diag::Analyzer(x) => x.is_error(),
            Diag::Cached(x) => x.is_error(),
        }
    }

    /// The source file owning this diagnostic, for cache attribution.
    fn path(&self) -> Option<PathBuf> {
        match self {
            Diag::Analyzer(x) => x
                .token_source()
                .get_path()
                .and_then(resource_table::get_path_value),
            Diag::Cached(x) => x.token_path().map(PathBuf::from),
        }
    }

    fn as_diagnostic(&self) -> &dyn Diagnostic {
        match self {
            Diag::Analyzer(x) => x,
            Diag::Cached(x) => x,
        }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Diag::Analyzer(x) => write!(f, "{x}"),
            Diag::Cached(x) => write!(f, "{x}"),
        }
    }
}

impl std::error::Error for Diag {}

impl Diagnostic for Diag {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.as_diagnostic().code()
    }
    fn severity(&self) -> Option<Severity> {
        self.as_diagnostic().severity()
    }
    fn help(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.as_diagnostic().help()
    }
    fn url(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.as_diagnostic().url()
    }
    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.as_diagnostic().source_code()
    }
    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.as_diagnostic().labels()
    }
    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        self.as_diagnostic().related()
    }
    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.as_diagnostic().diagnostic_source()
    }
}

#[derive(Error, Diagnostic, Debug)]
#[error("veryl check failed")]
pub struct CheckError {
    #[related]
    pub related: Vec<Diag>,
    error_count: u32,
    error_count_limit: u32,
}

impl CheckError {
    pub fn new(error_count_limit: u32) -> Self {
        Self {
            related: Vec::new(),
            error_count: 0,
            error_count_limit,
        }
    }

    pub fn append(mut self, x: &mut Vec<AnalyzerError>) -> Self {
        for x in x.drain(0..) {
            if !x.is_error() || self.error_count_limit == 0 {
                self.related.push(Diag::Analyzer(x));
            } else if self.error_count < self.error_count_limit {
                self.related.push(Diag::Analyzer(x));
                self.error_count += 1;
            }
        }
        self
    }

    /// Appends diagnostics restored from the incremental cache. These are
    /// always warnings (files with errors are never cached), so they bypass
    /// the error-count limit.
    pub fn append_cached(&mut self, diagnostics: Vec<CachedDiagnostic>) {
        self.related
            .extend(diagnostics.into_iter().map(Diag::Cached));
    }

    pub fn check_err(self) -> Result<Self> {
        if self.related.iter().all(|x| !x.is_error()) {
            Ok(self)
        } else {
            Err(self.into())
        }
    }

    pub fn check_all(self) -> Result<Self> {
        if self.related.is_empty() {
            Ok(self)
        } else {
            Err(self.into())
        }
    }

    /// `dump` passes `false` to keep analyzing a broken tree best-effort.
    fn check_err_if(self, fail_fast: bool) -> Result<Self> {
        if fail_fast {
            self.check_err()
        } else {
            Ok(self)
        }
    }
}

pub struct AnalyzeOutput {
    pub contexts: Vec<Context>,
    pub incremental: Option<Incremental>,
    pub check_error: CheckError,
    /// Files skipped from emit by the `--test` filter; the filelist omits them.
    pub filelist_excluded: HashSet<PathBuf>,
}

pub struct AnalyzeOptions<'a> {
    pub defines: &'a [String],
    /// A stale output demotes a cache hit; `true` only for emitting commands.
    pub emit_mode: bool,
    /// `false` for doc/dump/synth: they need every file's full pass2 IR/tables,
    /// which a restore (pass2 skipped) would leave incomplete.
    pub incremental: bool,
    /// `false` only for `dump`, which analyzes a broken tree best-effort.
    pub fail_fast: bool,
}

/// A supplied `ir` is populated by pass2 (the `veryl test` path); files holding
/// a selected test are then forced to miss so their IR is available.
pub fn analyze(
    metadata: &Metadata,
    paths: &[PathSet],
    opts: AnalyzeOptions<'_>,
    mut ir: Option<&mut veryl_analyzer::ir::Ir>,
    test_filter: Option<&str>,
) -> Result<AnalyzeOutput> {
    let mut check_error = CheckError::new(metadata.build.error_count_limit);
    let mut contexts = Vec::new();

    let mut stopwatch = StopWatch::new();

    // A selected test's file must miss: pass2 elaborates its instance tree from
    // the definition_table, which restored fragments also populate.
    let ir_requested = ir.is_some();
    let selected_tests = ir_requested.then_some(test_filter);
    let mut incremental = opts
        .incremental
        .then(|| {
            Incremental::open(
                metadata,
                paths,
                opts.defines,
                selected_tests,
                opts.emit_mode,
            )
        })
        .flatten();

    let analyzer = Analyzer::new(metadata);

    for path in paths {
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
        check_error = check_error
            .append(&mut errors)
            .check_err_if(opts.fail_fast)?;

        let context = Context::new(path.clone(), input, parser, analyzer.clone())?;
        contexts.push(context);
    }

    // Re-report warnings cached for restored files (their pass2 didn't run).
    if let Some(x) = incremental.as_mut() {
        check_error.append_cached(x.take_restored_diagnostics());
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
    check_error = check_error
        .append(&mut errors)
        .check_err_if(opts.fail_fast)?;

    debug!(
        "Executed analyze_post_pass1 ({} milliseconds)",
        stopwatch.lap()
    );

    // Skip pass2/emit for testbench files whose tests don't match `--test`;
    // they are never simulated. Matching ones stay unskipped for their IR.
    // Skipped files are collected so the filelist can omit their unemitted .sv.
    let mut filelist_excluded: HashSet<PathBuf> = HashSet::new();
    if ir_requested {
        let tests = veryl_analyzer::symbol_table::get_tests(&metadata.project.name);
        let mut test_file_ids: HashSet<PathId> = HashSet::new();
        let mut matching_file_ids: HashSet<PathId> = HashSet::new();
        for (name, prop) in &tests {
            test_file_ids.insert(prop.path);
            let name = name.to_string();
            if test_filter.is_none_or(|filter| name.contains(filter)) {
                matching_file_ids.insert(prop.path);
            }
        }
        let mut skipped = 0usize;
        for context in contexts.iter_mut() {
            let path_id = resource_table::insert_path(&context.path.src);
            if test_file_ids.contains(&path_id) && !matching_file_ids.contains(&path_id) {
                if !context.skip {
                    context.skip = true;
                    skipped += 1;
                }
                filelist_excluded.insert(context.path.src.clone());
            }
        }
        debug!(
            "test filter {:?}: skipped {} non-matching testbench files",
            test_filter, skipped
        );
    }

    let mut analyzer_context = veryl_analyzer::Context::default();

    for name in opts.defines {
        analyzer_context
            .config
            .defines
            .insert(resource_table::insert_str(name));
    }
    analyzer_context.enable_conv_profiler();

    // A local IR when the caller supplied none, so post-pass2's combinational-
    // loop check has something to inspect (cheap — it shares most work with pass2).
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
            check_error = check_error
                .append(&mut errors)
                .check_err_if(opts.fail_fast)?;
        }
    }

    debug!("Executed analyze_pass2 ({} milliseconds)", stopwatch.lap());
    analyzer_context.finalize_conv_profiler()?;

    let mut errors = Analyzer::analyze_post_pass2(ir_for_pass2);
    check_error = check_error
        .append(&mut errors)
        .check_err_if(opts.fail_fast)?;

    debug!(
        "Executed analyze_post_pass2 ({} milliseconds)",
        stopwatch.lap()
    );

    Ok(AnalyzeOutput {
        contexts,
        incremental,
        check_error,
        filelist_excluded,
    })
}

/// Freshly produced diagnostics grouped by the source file that owns them,
/// each flattened to a [`CachedDiagnostic`] for storage. A warm run reloads
/// these and re-reports the warning instead of re-running pass2.
///
/// `Cached` diagnostics are skipped: `Store::keep` already preserves a
/// restored file's blob.
///
/// A warning whose token resolves to no file (`Builtin`/`External`) can't be
/// cached, so it would vanish on a warm restore rather than be re-reported.
/// None exist today; one is logged at `debug` to surface the violation, so
/// anchor new warnings on user tokens.
pub fn collect_diagnosed(check_error: &CheckError) -> HashMap<PathBuf, Vec<CachedDiagnostic>> {
    let mut ret: HashMap<PathBuf, Vec<CachedDiagnostic>> = HashMap::new();
    for diag in &check_error.related {
        let Diag::Analyzer(error) = diag else {
            continue;
        };
        if let Some(path) = diag.path() {
            ret.entry(path)
                .or_default()
                .push(CachedDiagnostic::from_error(error));
        } else {
            debug!("diagnostic without a source path, not cached for warm restore: {error}");
        }
    }
    ret
}
