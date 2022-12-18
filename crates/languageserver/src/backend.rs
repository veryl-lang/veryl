use dashmap::DashMap;
use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use veryl_formatter::formatter::Formatter;
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_grammar_trait::Veryl;
use veryl_parser::veryl_parser::{miette, parse, ParserError};

#[derive(Debug)]
pub struct Backend {
    client: Client,
    document_map: DashMap<String, Rope>,
    ast_map: DashMap<String, Veryl>,
}

struct TextDocumentItem {
    uri: Url,
    text: String,
    version: i32,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            document_map: DashMap::new(),
            ast_map: DashMap::new(),
        }
    }

    async fn on_change(&self, params: TextDocumentItem) {
        let path = params.uri.to_string();
        let rope = Rope::from_str(&params.text);

        let mut grammar = VerylGrammar::new();

        let diag = match parse(&rope.to_string(), &path, &mut grammar) {
            Ok(_) => {
                if let Some(veryl) = grammar.veryl {
                    self.ast_map.insert(path.clone(), veryl);
                } else {
                    self.ast_map.remove(&path);
                }
                Vec::new()
            }
            Err(x) => {
                self.ast_map.remove(&path);
                Backend::to_diag(x, &rope)
            }
        };
        self.client
            .publish_diagnostics(params.uri, diag, Some(params.version))
            .await;

        self.document_map.insert(path, rope);
    }

    fn to_diag(err: miette::ErrReport, rope: &Rope) -> Vec<Diagnostic> {
        let miette_diag: &dyn miette::Diagnostic = err.as_ref();

        let range = if let Some(mut labels) = miette_diag.labels() {
            labels.next().map_or(Range::default(), |label| {
                let line = rope.byte_to_line(label.offset());
                let pos = label.offset() - rope.line_to_byte(line);
                let line = line as u32;
                let pos = pos as u32;
                let len = label.len() as u32;
                Range::new(Position::new(line, pos), Position::new(line, pos + len))
            })
        } else {
            Range::default()
        };

        let code = miette_diag
            .code()
            .map(|d| NumberOrString::String(format!("{d}")));

        let message = if let Some(x) = err.downcast_ref::<ParserError>() {
            match x {
                ParserError::PredictionErrorWithExpectations {
                    unexpected_tokens, ..
                } => {
                    format!(
                        "Syntax Error: {}",
                        Backend::demangle_unexpected_token(&unexpected_tokens[0].to_string())
                    )
                }
                _ => format!("Syntax Error: {}", x),
            }
        } else {
            format!("Syntax Error: {}", err)
        };

        let diag = Diagnostic::new(
            range,
            Some(DiagnosticSeverity::ERROR),
            code,
            Some(String::from("veryl-ls")),
            message,
            None,
            None,
        );
        vec![diag]
    }

    fn demangle_unexpected_token(text: &str) -> String {
        if text.contains("LBracketAMinusZ") {
            String::from("Unexpected token: Identifier")
        } else if text.contains("LBracket0Minus") {
            String::from("Unexpected token: Number")
        } else {
            text.replace("LA(1) (", "").replace(')', "")
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: String::from("veryl-ls"),
                version: Some(String::from(env!("CARGO_PKG_VERSION"))),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "server initialized!")
            .await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client.log_message(MessageType::INFO, "did_open").await;

        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.text_document.text,
            version: params.text_document.version,
        })
        .await
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_change")
            .await;

        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: std::mem::take(&mut params.content_changes[0].text),
            version: params.text_document.version,
        })
        .await
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let path = params.text_document.uri.to_string();
        if let Some(rope) = self.document_map.get(&path) {
            let line = rope.len_lines() as u32;
            if let Some(veryl) = self.ast_map.get(&path) {
                let mut formatter = Formatter::new();
                formatter.format(&veryl);

                let text_edit = TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(line, u32::MAX)),
                    new_text: formatter.as_str().to_string(),
                };

                return Ok(Some(vec![text_edit]));
            }
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
