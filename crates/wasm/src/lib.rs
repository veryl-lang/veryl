use miette::{ErrReport, GraphicalReportHandler, GraphicalTheme, ThemeCharacters, ThemeStyles};
use semver::Version;
use std::collections::HashMap;
use std::path::PathBuf;
use veryl_analyzer::{namespace_table, symbol_table, Analyzer};
use veryl_emitter::Emitter;
use veryl_formatter::Formatter;
use veryl_metadata::{Build, Doc, Format, Lint, Lockfile, Metadata, Project, Pubfile, Publish};
use veryl_parser::{resource_table, Parser};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub fn alert(s: &str);
}

#[wasm_bindgen]
pub struct Result {
    err: bool,
    content: String,
}

#[wasm_bindgen]
impl Result {
    #[wasm_bindgen]
    pub fn err(&self) -> bool {
        self.err
    }

    #[wasm_bindgen]
    pub fn content(&self) -> String {
        self.content.clone()
    }
}

fn render_err(err: ErrReport) -> String {
    let mut out = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme {
        characters: ThemeCharacters::emoji(),
        styles: ThemeStyles::none(),
    })
    .with_width(80)
    .render_report(&mut out, err.as_ref())
    .unwrap();
    out
}

fn metadata() -> Metadata {
    Metadata {
        project: Project {
            name: "project".into(),
            version: Version::parse("0.0.0").unwrap(),
            authors: vec![],
            description: None,
            license: None,
            repository: None,
        },
        build: Build::default(),
        format: Format::default(),
        lint: Lint::default(),
        publish: Publish::default(),
        doc: Doc::default(),
        dependencies: HashMap::new(),
        metadata_path: "".into(),
        pubfile_path: "".into(),
        pubfile: Pubfile::default(),
        lockfile_path: "".into(),
        lockfile: Lockfile::default(),
    }
}

#[wasm_bindgen]
pub fn build(source: &str) -> Result {
    let metadata = metadata();
    match Parser::parse(source, &"") {
        Ok(parser) => {
            if let Some(path) = resource_table::get_path_id(PathBuf::from("")) {
                symbol_table::drop(path);
                namespace_table::drop(path);
            }

            let analyzer = Analyzer::new(&metadata);
            let mut errors = Vec::new();
            errors.append(&mut analyzer.analyze_pass1("project", source, "", &parser.veryl));
            errors.append(&mut analyzer.analyze_pass2("project", source, "", &parser.veryl));
            errors.append(&mut analyzer.analyze_pass3("project", source, "", &parser.veryl));

            let err = !errors.is_empty();

            let content = if err {
                let mut text = String::new();
                for e in errors {
                    text.push_str(&render_err(e.into()));
                }
                text
            } else {
                let mut emitter = Emitter::new(&metadata);
                emitter.emit("project", &parser.veryl);
                emitter.as_str().to_owned()
            };

            Result { err, content }
        }
        Err(e) => Result {
            err: true,
            content: render_err(e.into()),
        },
    }
}

#[wasm_bindgen]
pub fn format(source: &str) -> Result {
    let metadata = metadata();
    match Parser::parse(source, &"") {
        Ok(parser) => {
            let mut formatter = Formatter::new(&metadata);
            formatter.format(&parser.veryl);
            Result {
                err: false,
                content: formatter.as_str().to_owned(),
            }
        }
        Err(e) => Result {
            err: true,
            content: render_err(e.into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn get_default_code() -> String {
        let path = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let mut path = PathBuf::from(path);
        path.push("playground");
        path.push("index.html");
        let text = std::fs::read_to_string(path).unwrap();
        let mut code = false;
        let mut code_text = String::new();
        for line in text.lines() {
            if line.contains("</textarea") {
                code = false;
            }
            if code {
                code_text.push_str(&format!("{line}\n"));
            }
            if line.contains("<textarea") {
                code = true;
            }
        }
        code_text
    }

    #[test]
    fn build_default_code() {
        let text = get_default_code();
        let ret = build(&text);

        assert_eq!(ret.err, false);
        assert_ne!(ret.content, "");
    }

    #[test]
    fn format_default_code() {
        let text = get_default_code();
        let ret = format(&text);

        assert_eq!(ret.err, false);
        assert_eq!(ret.content, text);
    }
}
