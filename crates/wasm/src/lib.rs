use semver::Version;
use veryl_emitter::Emitter;
use veryl_formatter::Formatter;
use veryl_metadata::{Build, Format, Metadata, Package};
use veryl_parser::miette::{
    ErrReport, GraphicalReportHandler, GraphicalTheme, ThemeCharacters, ThemeStyles,
};
use veryl_parser::Parser;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub fn alert(s: &str);
}

#[wasm_bindgen]
pub struct ParseResult {
    code: String,
    err: String,
}

#[wasm_bindgen]
impl ParseResult {
    #[wasm_bindgen]
    pub fn code(&self) -> String {
        self.code.clone()
    }

    #[wasm_bindgen]
    pub fn err(&self) -> String {
        self.err.clone()
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
        package: Package {
            name: "".into(),
            version: Version::parse("0.0.0").unwrap(),
            authors: vec![],
            description: None,
            license: None,
            repository: None,
        },
        build: Build::default(),
        format: Format::default(),
        metadata_path: "".into(),
    }
}

#[wasm_bindgen]
pub fn parse(source: &str) -> ParseResult {
    let metadata = metadata();
    match Parser::parse(source, &"") {
        Ok(parser) => {
            let mut emitter = Emitter::new(&metadata);
            emitter.emit(&parser.veryl);
            ParseResult {
                code: emitter.as_str().to_owned(),
                err: "".to_owned(),
            }
        }
        Err(e) => ParseResult {
            code: "".to_owned(),
            err: format!("{}", render_err(e)),
        },
    }
}

#[wasm_bindgen]
pub fn format(source: &str) -> ParseResult {
    let metadata = metadata();
    match Parser::parse(source, &"") {
        Ok(parser) => {
            let mut formatter = Formatter::new(&metadata);
            formatter.format(&parser.veryl);
            ParseResult {
                code: formatter.as_str().to_owned(),
                err: "".to_owned(),
            }
        }
        Err(e) => ParseResult {
            code: "".to_owned(),
            err: format!("{}", render_err(e)),
        },
    }
}
