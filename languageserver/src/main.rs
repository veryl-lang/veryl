use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use veryl_parser::veryl_grammar::VerylGrammar;
use veryl_parser::veryl_parser::parse;

#[derive(Debug)]
struct Backend {
    client: Client,
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

        let text = params.text_document.text;
        let path = params.text_document.uri.path().to_string();

        let mut grammar = VerylGrammar::new();
        if let Ok(_) = parse(&text, &path, &mut grammar) {}
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_change")
            .await;

        let text = &params.content_changes[0].text;
        let path = params.text_document.uri.path().to_string();

        let mut grammar = VerylGrammar::new();
        if let Ok(_) = parse(&text, &path, &mut grammar) {}
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend { client });

    Server::new(stdin, stdout, socket).serve(service).await;
}
