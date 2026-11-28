use crate::incremental::OutputIntent;
use crate::pipeline::{self, AnalyzeOptions, AnalyzeOutput, CheckError, Diag};
use crate::{Format, OptCheck, check_format_version};
use miette::{Diagnostic, IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use veryl_metadata::Metadata;

/// Machine-readable diagnostic entry emitted by `veryl check --json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Machine-readable report emitted by `veryl check --json` or `veryl check --format json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// Report schema version. Currently 1.
    pub format_version: u32,
    pub success: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub diagnostics: Vec<CheckDiagnostic>,
}

impl CheckDiagnostic {
    pub fn from_diag(diag: &Diag, root_path: Option<&Path>) -> Self {
        let severity = if diag.is_error() {
            "error".to_string()
        } else {
            "warning".to_string()
        };
        let code = diag.code().map(|x| x.to_string());
        let help = diag.help().map(|x| x.to_string());
        let url = diag.url().map(|x| x.to_string());
        let message = diag.to_string();

        let (path, line, column) = match (diag.labels(), diag.source_code()) {
            (Some(mut labels), Some(source_code)) => {
                if let Some(first_label) = labels.next() {
                    if let Ok(sc) = source_code.read_span(first_label.inner(), 0, 0) {
                        let path = sc.name().map(|name| {
                            let path_buf = Path::new(name);
                            let clean_path = root_path
                                .map_or(path_buf, |r| path_buf.strip_prefix(r).unwrap_or(path_buf));
                            clean_path.to_string_lossy().replace('\\', "/")
                        });
                        (path, Some(sc.line() + 1), Some(sc.column() + 1))
                    } else {
                        (None, None, None)
                    }
                } else {
                    (None, None, None)
                }
            }
            _ => (None, None, None),
        };

        let path = path.or_else(|| {
            diag.path().map(|p| {
                let p_ref = p.as_path();
                let clean_path =
                    root_path.map_or(p_ref, |r| p_ref.strip_prefix(r).unwrap_or(p_ref));
                clean_path.to_string_lossy().replace('\\', "/")
            })
        });

        Self {
            code,
            severity,
            message,
            path,
            line,
            column,
            help,
            url,
        }
    }
}

impl CheckReport {
    pub fn from_check_error(check_error: &CheckError, root_path: Option<&Path>) -> Self {
        let mut diagnostics = Vec::with_capacity(check_error.related.len());
        let mut error_count = 0;
        let mut warning_count = 0;

        for diag in &check_error.related {
            if diag.is_error() {
                error_count += 1;
            } else {
                warning_count += 1;
            }
            diagnostics.push(CheckDiagnostic::from_diag(diag, root_path));
        }

        Self {
            format_version: 1,
            success: error_count == 0 && warning_count == 0,
            error_count,
            warning_count,
            diagnostics,
        }
    }
}

pub struct CmdCheck {
    opt: OptCheck,
}

impl CmdCheck {
    pub fn new(opt: OptCheck) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let is_json = matches!(self.opt.format, Format::Json);
        check_format_version(self.opt.format, self.opt.format_version)?;

        let paths = metadata.paths(&self.opt.files, true, true)?;

        let options = AnalyzeOptions {
            defines: &[],
            output_intent: OutputIntent::Never,
            incremental: true,
            fail_fast: !is_json,
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

        if is_json {
            let project_path = metadata.project_path();
            let report = CheckReport::from_check_error(&check_error, Some(&project_path));
            let json_str = serde_json::to_string_pretty(&report).into_diagnostic()?;
            println!("{json_str}");
            Ok(report.success)
        } else {
            // check fails on warnings, not just errors.
            let _ = check_error.check_all()?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TEST_TOML: &str = r#"
[project]
name = "test_check"
version = "0.1.0"
"#;

    fn setup_test_project() -> (Metadata, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().unwrap();
        let project_dir = tempdir.path().join("test_check");
        fs::create_dir_all(project_dir.join("src")).unwrap();
        let toml_path = project_dir.join("Veryl.toml");
        fs::write(&toml_path, TEST_TOML).unwrap();
        let metadata = Metadata::load(&toml_path).unwrap();
        (metadata, tempdir)
    }

    #[test]
    fn check_json_clean_project_reports_success() {
        let (mut metadata, _tempdir) = setup_test_project();
        let src_file = metadata.project_path().join("src").join("valid.veryl");
        fs::write(
            &src_file,
            r#"
module ModuleA {
    let _a: logic = 1'b0;
}
"#,
        )
        .unwrap();

        let opt = OptCheck {
            files: Vec::new(),
            format: Format::Json,
            format_version: None,
        };
        let cmd = CmdCheck::new(opt);
        let ret = cmd.exec(&mut metadata);
        assert!(ret.is_ok());
        assert!(ret.unwrap());
    }

    #[test]
    fn check_json_with_error_reports_failure() {
        let (mut metadata, _tempdir) = setup_test_project();
        let src_file = metadata.project_path().join("src").join("invalid.veryl");
        fs::write(
            &src_file,
            r#"
module ModuleInvalid {
    assign a = 1'b0;
}
"#,
        )
        .unwrap();

        let opt = OptCheck {
            files: Vec::new(),
            format: Format::Json,
            format_version: None,
        };
        let cmd = CmdCheck::new(opt);
        let ret = cmd.exec(&mut metadata);
        assert!(ret.is_ok());
        assert!(!ret.unwrap(), "should return false on check failures");
    }

    #[test]
    fn check_format_version_validation() {
        let (mut metadata, _tempdir) = setup_test_project();
        let opt_bad_ver = OptCheck {
            files: Vec::new(),
            format: Format::Json,
            format_version: Some(2),
        };
        let cmd = CmdCheck::new(opt_bad_ver);
        let err = cmd.exec(&mut metadata).unwrap_err();
        assert!(err.to_string().contains("unsupported --format-version 2"));

        let opt_pretty_ver = OptCheck {
            files: Vec::new(),
            format: Format::Pretty,
            format_version: Some(1),
        };
        let cmd_pretty = CmdCheck::new(opt_pretty_ver);
        let err_pretty = cmd_pretty.exec(&mut metadata).unwrap_err();
        assert!(
            err_pretty
                .to_string()
                .contains("--format-version is only supported with --format json")
        );
    }
}
