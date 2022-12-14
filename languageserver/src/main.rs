use dashmap::DashMap;
use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use veryl_parser::formatter::Formatter;
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_grammar_trait::Veryl;
use veryl_parser::veryl_parser::parse;

#[derive(Debug)]
struct Backend {
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
    async fn on_change(&self, params: TextDocumentItem) {
        let path = params.uri.to_string();
        let rope = Rope::from_str(&params.text);

        let mut grammar = VerylGrammar::new();
        if let Ok(_) = parse(&rope.to_string(), &path, &mut grammar) {
            if let Some(veryl) = grammar.veryl {
                self.ast_map.insert(path.clone(), veryl);
            } else {
                self.ast_map.remove(&path);
            }
        } else {
            self.ast_map.remove(&path);
        }

        self.document_map.insert(path, rope);
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

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let document_map = DashMap::new();
    let ast_map = DashMap::new();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        document_map,
        ast_map,
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}
