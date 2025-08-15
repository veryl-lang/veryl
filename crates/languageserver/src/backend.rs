use std::str::FromStr;

use crate::server::{
    Capability, MsgFromServer, MsgToServer, Server, ServerConfigItem, semantic_legend,
};
use async_channel::{Receiver, Sender, unbounded};
use serde_json::Value;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::lsp_types::Uri as Url;
use tower_lsp_server::lsp_types::*;
use tower_lsp_server::{Client, LanguageServer};

const COMPLETION_TRIGGER: &[&str] = &["<", ">", "=", "!", "."];

#[derive(Debug)]
pub struct Backend {
    client: Client,
    rcv: Receiver<MsgFromServer>,
    snd: Sender<MsgToServer>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let (tx_from, rx_from) = unbounded();
        let (tx_to, rx_to) = unbounded();
        let mut server = Server::new(client.clone(), rx_to, tx_from);
        std::thread::spawn(move || server.serve());

        Self {
            client,
            rcv: rx_from,
            snd: tx_to,
        }
    }

    async fn send(&self, msg: MsgToServer) {
        if let Err(x) = self.snd.send(msg).await {
            self.client.log_message(MessageType::ERROR, x).await;
        }
    }

    async fn recv(&self) -> Option<MsgFromServer> {
        match self.rcv.recv().await {
            Ok(x) => Some(x),
            Err(x) => {
                self.client.log_message(MessageType::ERROR, x).await;
                None
            }
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let capability: Capability = params.capabilities.into();
        self.send(MsgToServer::Initialize { capability }).await;

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
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        did_create: None,
                        will_create: None,
                        did_rename: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.veryl".to_string(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                        will_rename: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.veryl".to_string(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                        did_delete: None,
                        will_delete: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.veryl".to_string(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                    }),
                }),
                definition_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions {
                                work_done_progress: Some(false),
                            },
                            legend: SemanticTokensLegend {
                                token_types: semantic_legend::get_token_types(),
                                token_modifiers: semantic_legend::get_token_modifiers(),
                            },
                            range: Some(false),
                            full: Some(SemanticTokensFullOptions::Delta { delta: Some(false) }),
                        },
                    ),
                ),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(
                        COMPLETION_TRIGGER.iter().map(|x| x.to_string()).collect(),
                    ),
                    all_commit_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    completion_item: None,
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

        let url = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        self.send(MsgToServer::DidOpen { url, text, version }).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_change")
            .await;

        let url = params.text_document.uri;
        let text = std::mem::take(&mut params.content_changes[0].text);
        let version = params.text_document.version;

        self.send(MsgToServer::DidChange { url, text, version })
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("did_change_watched_files: {change:?}"),
                )
                .await;
        }
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Value::Object(x) = params.settings
            && let Some(x) = x.get("veryl-ls")
            && let Some(Value::Bool(x)) = x.get("useOperatorCompletion")
        {
            let x = ServerConfigItem::UseOperatorCompletion(*x);
            self.send(MsgToServer::DidChangeConfiguration(x)).await;
        }
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        self.client
            .log_message(MessageType::INFO, "will_rename_files")
            .await;

        for file in params.files {
            if let Ok(url) = Url::from_str(file.old_uri.as_str()) {
                self.send(MsgToServer::WillRenameFile { old_url: url })
                    .await;
            }
        }
        Ok(None)
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        self.client
            .log_message(MessageType::INFO, "did_rename_files")
            .await;

        // Currently it only triggers a background analysis, so no need to send all the uris.
        for file in params.files {
            if let Ok(url) = Url::from_str(file.old_uri.as_str()) {
                self.send(MsgToServer::DidRenameFile { new_url: url }).await;
                break;
            }
        }
    }

    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        self.client
            .log_message(MessageType::INFO, "will_delete_files")
            .await;

        for file in params.files {
            if let Ok(url) = Url::from_str(file.uri.as_str()) {
                self.send(MsgToServer::WillDeleteFile { url }).await;
            }
        }
        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let url = params.text_document_position.text_document.uri;
        let line = params.text_document_position.position.line as usize + 1;
        let column = params.text_document_position.position.character as usize + 1;
        let context = params.context;

        self.send(MsgToServer::Completion {
            url,
            line,
            column,
            context,
        })
        .await;

        if let Some(MsgFromServer::Completion(x)) = self.recv().await {
            Ok(x)
        } else {
            Ok(None)
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let url = params.text_document_position_params.text_document.uri;
        let line = params.text_document_position_params.position.line as usize + 1;
        let column = params.text_document_position_params.position.character as usize + 1;

        self.send(MsgToServer::GotoDefinition { url, line, column })
            .await;

        if let Some(MsgFromServer::GotoDefinition(Some(x))) = self.recv().await {
            Ok(Some(GotoDefinitionResponse::Scalar(x)))
        } else {
            Ok(None)
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<OneOf<Vec<SymbolInformation>, Vec<WorkspaceSymbol>>>> {
        let query = params.query;

        self.send(MsgToServer::Symbol { query }).await;

        if let Some(MsgFromServer::Symbol(x)) = self.recv().await {
            Ok(Some(OneOf::Left(x)))
        } else {
            Ok(None)
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let url = params.text_document_position_params.text_document.uri;
        let line = params.text_document_position_params.position.line as usize + 1;
        let column = params.text_document_position_params.position.character as usize + 1;

        self.send(MsgToServer::Hover { url, line, column }).await;

        if let Some(MsgFromServer::Hover(Some(x))) = self.recv().await {
            Ok(Some(x))
        } else {
            Ok(None)
        }
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let url = params.text_document_position.text_document.uri;
        let line = params.text_document_position.position.line as usize + 1;
        let column = params.text_document_position.position.character as usize + 1;

        self.send(MsgToServer::References { url, line, column })
            .await;

        if let Some(MsgFromServer::References(x)) = self.recv().await {
            Ok(Some(x))
        } else {
            Ok(None)
        }
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let url = params.text_document.uri;

        self.send(MsgToServer::SemanticTokens { url }).await;

        if let Some(MsgFromServer::SemanticTokens(x)) = self.recv().await {
            Ok(x)
        } else {
            Ok(None)
        }
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let url = params.text_document.uri;

        self.send(MsgToServer::Formatting { url }).await;

        if let Some(MsgFromServer::Formatting(x)) = self.recv().await {
            Ok(x)
        } else {
            Ok(None)
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
