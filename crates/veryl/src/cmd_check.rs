use crate::OptCheck;
use crate::incremental::OutputIntent;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput, CheckError, Diag};
use crate::utils;
use log::info;
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use veryl_analyzer::AnalyzerError;
use veryl_metadata::Metadata;

pub struct CmdCheck {
    opt: OptCheck,
}

struct Patch {
    offset: usize,
    len: usize,
    replacement: String,
}

impl CmdCheck {
    pub fn new(opt: OptCheck) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        let options = AnalyzeOptions {
            defines: &[],
            output_intent: OutputIntent::Never,
            incremental: true,
            fail_fast: !self.opt.fix,
        };
        let AnalyzeOutput {
            incremental,
            check_error,
            ..
        } = pipeline::analyze(metadata, &paths, options, None, None)?;

        // Save clean files before failing on warnings, so a second check warms.
        if let Some(mut inc) = incremental {
            inc.save(&pipeline::collect_diagnosed(&check_error));
        }

        if self.opt.fix {
            let patches = collect_patches(&check_error);
            let count = apply_patches(patches)?;
            if count > 0 {
                info!("Applied {count} fixes");
                veryl_analyzer::Analyzer::new(metadata).clear();
                // Re-run check after applying fixes to verify
                let recheck_options = AnalyzeOptions {
                    defines: &[],
                    output_intent: OutputIntent::Never,
                    incremental: false,
                    fail_fast: true,
                };
                let recheck_output =
                    pipeline::analyze(metadata, &paths, recheck_options, None, None)?;
                let _ = recheck_output.check_error.check_all()?;
                return Ok(true);
            }
        }

        // check fails on warnings, not just errors.
        let _ = check_error.check_all()?;
        Ok(true)
    }
}

fn collect_patches(check_error: &CheckError) -> HashMap<PathBuf, Vec<Patch>> {
    let mut patches: HashMap<PathBuf, Vec<Patch>> = HashMap::new();
    for diag in &check_error.related {
        if let Diag::Analyzer(error) = diag {
            match error {
                AnalyzerError::UnusedVariable {
                    identifier,
                    error_location,
                    token_source,
                    ..
                } => {
                    if can_rename_unused_variable(identifier, error_location, token_source)
                        && let Some(path) = token_source
                            .get_path()
                            .and_then(veryl_parser::resource_table::get_path_value)
                    {
                        patches.entry(path).or_default().push(Patch {
                            offset: error_location.offset(),
                            len: error_location.len(),
                            replacement: format!("_{identifier}"),
                        });
                    }
                }
                AnalyzerError::WrongSeparator {
                    valid_separator,
                    error_location,
                    token_source,
                    ..
                } => {
                    if let Some(path) = token_source
                        .get_path()
                        .and_then(veryl_parser::resource_table::get_path_value)
                    {
                        patches.entry(path).or_default().push(Patch {
                            offset: error_location.offset(),
                            len: error_location.len(),
                            replacement: valid_separator.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    patches
}

fn apply_patches(patches_by_file: HashMap<PathBuf, Vec<Patch>>) -> Result<usize> {
    let mut total_applied = 0;
    for (path, mut file_patches) in patches_by_file {
        if file_patches.is_empty() {
            continue;
        }
        // Sort descending by offset so edits at end of file preserve earlier offsets
        file_patches.sort_by_key(|p| std::cmp::Reverse(p.offset));

        let content = fs::read_to_string(&path)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to read file: {}", path.to_string_lossy()))?;
        let mut modified = content;

        for patch in &file_patches {
            if patch.offset <= modified.len() && patch.offset + patch.len <= modified.len() {
                modified.replace_range(patch.offset..patch.offset + patch.len, &patch.replacement);
                total_applied += 1;
            }
        }

        utils::write_file_if_changed(&path, modified.as_bytes())?;
    }
    Ok(total_applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOML: &str = r#"
[project]
name = "test_check_fix"
version = "0.1.0"
"#;

    fn setup_test_project() -> (Metadata, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().unwrap();
        let project_dir = tempdir.path().join("test_check_fix");
        fs::create_dir_all(project_dir.join("src")).unwrap();
        let toml_path = project_dir.join("Veryl.toml");
        fs::write(&toml_path, TEST_TOML).unwrap();
        let metadata = Metadata::load(&toml_path).unwrap();
        (metadata, tempdir)
    }

    #[test]
    fn check_fix_unused_variable() {
        let (mut metadata, _tempdir) = setup_test_project();
        let src_file = metadata.project_path().join("src").join("unused.veryl");
        fs::write(
            &src_file,
            r#"
module ModuleUnused {
    let a: logic = 1'b0;
}
"#,
        )
        .unwrap();

        let opt = OptCheck {
            files: Vec::new(),
            fix: true,
        };
        let cmd = CmdCheck::new(opt);
        let ret = cmd.exec(&mut metadata);
        assert!(ret.is_ok(), "fix should succeed: {:?}", ret.err());
        assert!(ret.unwrap());

        let fixed_content = fs::read_to_string(&src_file).unwrap();
        assert!(
            fixed_content.contains("let _a: logic = 1'b0;"),
            "content was: {fixed_content}"
        );
    }

    #[test]
    fn check_fix_skips_unused_variable_when_renamed_symbol_visible() {
        let (mut metadata, _tempdir) = setup_test_project();
        let src_file = metadata.project_path().join("src").join("collision.veryl");
        fs::write(
            &src_file,
            r#"
module ModuleA (
    _a: input  logic,
    o : output logic,
) {
    always_comb {
        let a: logic = 0;
        o = _a;
    }
}
"#,
        )
        .unwrap();

        let opt = OptCheck {
            files: Vec::new(),
            fix: true,
        };
        let cmd = CmdCheck::new(opt);
        let ret = cmd.exec(&mut metadata);
        assert!(
            ret.is_err(),
            "unfixed unused warning should still fail check"
        );

        let content = fs::read_to_string(&src_file).unwrap();
        assert!(
            content.contains("let a: logic = 0;"),
            "colliding rename should be skipped, content was: {content}"
        );
    }
}

fn can_rename_unused_variable(
    identifier: &str,
    error_location: &miette::SourceSpan,
    token_source: &veryl_parser::veryl_token::TokenSource,
) -> bool {
    let renamed = format!("_{identifier}");
    let renamed_id = veryl_parser::resource_table::insert_str(&renamed);
    let path = veryl_analyzer::symbol_path::SymbolPath::new(&[renamed_id]);
    let offset = error_location.offset() as u32;

    let all_symbols = veryl_analyzer::symbol_table::get_all();
    let Some(symbol) = all_symbols
        .iter()
        .find(|s| s.token.source == *token_source && s.token.pos == offset)
    else {
        return true;
    };

    // Reject the rename if `_<identifier>` is already visible in this scope.
    veryl_analyzer::symbol_table::resolve((&path, &symbol.namespace)).is_err()
}
