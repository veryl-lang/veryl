#![allow(unnameable_test_items)]

use crate::Backend;
use serde_json::{Value, json};
use std::env;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tower_lsp_server::jsonrpc::{Id, Request, Response};
use tower_lsp_server::ls_types::Uri as Url;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

struct TestServer {
    req_stream: DuplexStream,
    res_stream: DuplexStream,
    recv_buf: Vec<u8>,
}

impl TestServer {
    fn new<F, S>(init: F) -> Self
    where
        F: FnOnce(Client) -> S,
        S: LanguageServer,
    {
        let (req_client, req_server) = tokio::io::duplex(1024);
        let (res_server, res_client) = tokio::io::duplex(1024);

        let (service, socket) = LspService::new(init);

        tokio::spawn(Server::new(req_server, res_server, socket).serve(service));

        Self {
            req_stream: req_client,
            res_stream: res_client,
            recv_buf: Vec::new(),
        }
    }

    fn encode(payload: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload)
    }

    fn try_decode_one(buf: &mut Vec<u8>) -> Option<String> {
        let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let header = std::str::from_utf8(&buf[..header_end]).unwrap();
        let len: usize = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();
        let body_start = header_end + 4;
        let body_end = body_start + len;
        if buf.len() < body_end {
            return None;
        }
        let body = String::from_utf8(buf[body_start..body_end].to_vec()).unwrap();
        buf.drain(..body_end);
        Some(body)
    }

    async fn send_request(&mut self, req: Request) {
        let req = serde_json::to_string(&req).unwrap();
        let req = Self::encode(&req);
        self.req_stream.write_all(req.as_bytes()).await.unwrap();
    }

    async fn send_ack(&mut self, id: &Id) {
        let req = Response::from_ok(id.clone(), None::<serde_json::Value>.into());
        let req = serde_json::to_string(&req).unwrap();
        let req = Self::encode(&req);
        self.req_stream.write_all(req.as_bytes()).await.unwrap();
    }

    async fn recv_message(&mut self) -> String {
        loop {
            if let Some(msg) = Self::try_decode_one(&mut self.recv_buf) {
                return msg;
            }
            let mut tmp = [0u8; 4096];
            let n = self.res_stream.read(&mut tmp).await.unwrap();
            assert!(n > 0, "LSP stream closed unexpectedly");
            self.recv_buf.extend_from_slice(&tmp[..n]);
        }
    }

    async fn recv_response(&mut self) -> Response {
        let res = self.recv_message().await;
        serde_json::from_str(&res).unwrap()
    }

    async fn recv_notification(&mut self) -> Request {
        let res = self.recv_message().await;
        serde_json::from_str(&res).unwrap()
    }

    /// Receives until a response arrives, acknowledging interleaved progress requests.
    async fn recv_response_skipping(&mut self) -> Value {
        loop {
            let msg = self.recv_message().await;
            let value: Value = serde_json::from_str(&msg).unwrap();
            if value.get("result").is_some() {
                return value;
            }
            if value["method"] == "window/workDoneProgress/create" {
                let request: Request = serde_json::from_str(&msg).unwrap();
                self.send_ack(request.id().unwrap()).await;
            }
        }
    }
}

fn build_initialize(id: i64) -> Request {
    let mut params = InitializeParams::default();
    params.capabilities.window = Some(WindowClientCapabilities {
        work_done_progress: Some(true),
        ..Default::default()
    });
    Request::build("initialize")
        .params(json!(params))
        .id(id)
        .finish()
}

fn build_initialized() -> Request {
    let params = InitializedParams {};
    Request::build("initialized").params(json!(params)).finish()
}

#[tokio::test]
#[ntest::timeout(10000)]
async fn semantic_tokens_refresh_after_editing_a_declaration_file() {
    check_semantic_tokens_after_declaration_edit(
        "package Types { const WIDTH: u32 = 8; }",
        "package Types {}",
        "module Top {
    import Types::WIDTH;
    let _qualified: u32 = Types::WIDTH;
    let _imported: u32 = WIDTH;
}",
        &[1, 2, 3],
        (Position::new(3, 26), 0),
        Some("module Disk { let _n: u32 = Types::WIDTH; }"),
    )
    .await;
}

#[tokio::test]
#[ntest::timeout(10000)]
async fn semantic_tokens_refresh_after_editing_a_generic_declaration_file() {
    check_semantic_tokens_after_declaration_edit(
        "package Types::<N: u32> { const WIDTH: u32 = N; }",
        "package Other {}",
        "module Top {
    let _qualified: u32 = Types::<8>::WIDTH;
}",
        &[1],
        (Position::new(1, 40), 0),
        None,
    )
    .await;
}

#[tokio::test]
#[ntest::timeout(10000)]
async fn project_references_follow_transitive_imports_after_editing() {
    check_semantic_tokens_after_declaration_edit(
        "package Target { const WIDTH: u32 = 32; }
package Actual { const WIDTH: u32 = 8; }
package Group { alias package Target = Actual; }",
        "package Target { const WIDTH: u32 = 32; }
package Actual {}
package Group { alias package Target = Actual; }",
        "module Top {
    import Bridge::Selected::*;
    let _qualified: u32 = Bridge::Selected::WIDTH;
    let _imported: u32 = WIDTH;
}",
        &[2, 3],
        (Position::new(3, 26), 1),
        Some(
            "package Bridge {
    import Group::Target;
    alias package Selected = Target;
    const USE: u32 = Selected::WIDTH;
}",
        ),
    )
    .await;
}

#[tokio::test]
#[ntest::timeout(10000)]
async fn watched_sources_update_references_without_reloading_the_project() {
    let project = tempfile::tempdir().unwrap();
    let root = editor_project_root(&project);
    std::fs::write(
        root.join("Veryl.toml"),
        "[project]\nname = \"watched_test\"\nversion = \"0.1.0\"\n[build]\nexclude_std = true\nincremental = false\n",
    )
    .unwrap();
    let definitions = root.join("definitions.veryl");
    let definitions_uri = Url::from_file_path(&definitions).unwrap();
    let references = "module Top { let _value: u32 = Types::WIDTH; }";
    let references_file = root.join("references.veryl");
    std::fs::write(&references_file, references).unwrap();
    std::fs::write(root.join("unrelated.veryl"), "module Unrelated {}").unwrap();
    let references_uri = Url::from_file_path(references_file).unwrap();
    let provider = "package Types { const WIDTH: u32 = 8; }";

    let mut server = TestServer::new(Backend::new);
    server
        .send_request(
            Request::build("initialize")
                .params(json!({"capabilities": {"workspace": {
                    "semanticTokens": {"refreshSupport": true},
                    "didChangeWatchedFiles": {"dynamicRegistration": true}
                }}}))
                .id(1)
                .finish(),
        )
        .await;
    let initialize = server.recv_response_skipping().await;
    let variable =
        initialize["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|token| token == "variable")
            .unwrap() as u32;
    server.send_request(build_initialized()).await;
    loop {
        let request = server.recv_notification().await;
        if request.method() == "client/registerCapability" {
            let registration = &request.params().unwrap()["registrations"][0];
            assert_eq!(registration["method"], "workspace/didChangeWatchedFiles");
            assert_eq!(
                registration["registerOptions"]["watchers"][0]["globPattern"],
                "**/*.veryl"
            );
            server.send_ack(request.id().unwrap()).await;
            break;
        }
    }

    for (uri, text) in [(&references_uri, references), (&definitions_uri, provider)] {
        server
            .send_request(
                Request::build("textDocument/didOpen")
                    .params(json!({"textDocument": {"uri": uri, "languageId": "veryl", "version": 1, "text": text}}))
                    .finish(),
            )
            .await;
        let analyzed = wait_for_refresh(&mut server).await;
        if uri == &references_uri {
            assert_eq!(analyzed, 1);
            assert!(
                semantic_lines(&mut server, &references_uri, references, variable)
                    .await
                    .is_empty()
            );
        } else {
            assert_eq!(analyzed, 0);
            assert_eq!(
                semantic_lines(&mut server, &references_uri, references, variable).await,
                [0]
            );
        }
    }

    // Saving externally while the editor is open leaves its buffer authoritative.
    std::fs::write(&definitions, "package Types {}").unwrap();
    let watched = |typ| {
        Request::build("workspace/didChangeWatchedFiles")
            .params(json!({"changes": [{"uri": definitions_uri, "type": typ}]}))
            .finish()
    };
    server.send_request(watched(FileChangeType::CREATED)).await;
    assert_eq!(
        semantic_lines(&mut server, &references_uri, references, variable).await,
        [0]
    );
    server
        .send_request(
            Request::build("textDocument/didClose")
                .params(json!({"textDocument": {"uri": definitions_uri}}))
                .finish(),
        )
        .await;
    assert_eq!(wait_for_refresh(&mut server).await, 0);
    assert!(
        semantic_lines(&mut server, &references_uri, references, variable)
            .await
            .is_empty()
    );

    for (typ, contents, highlighted) in [
        (FileChangeType::CHANGED, Some(provider), true),
        (FileChangeType::CHANGED, Some("package"), false),
        (FileChangeType::CHANGED, Some(provider), true),
        (FileChangeType::DELETED, None, false),
        (FileChangeType::CREATED, Some(provider), true),
        (FileChangeType::DELETED, None, false),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&definitions, contents).unwrap();
        } else {
            std::fs::remove_file(&definitions).unwrap();
        }
        server.send_request(watched(typ)).await;
        assert_eq!(wait_for_refresh(&mut server).await, 0);
        assert_eq!(
            !semantic_lines(&mut server, &references_uri, references, variable)
                .await
                .is_empty(),
            highlighted,
        );
    }

    let old_path = root.join("renamed-from.veryl");
    let old_uri = Url::from_file_path(&old_path).unwrap();
    std::fs::write(&old_path, provider).unwrap();
    server
        .send_request(
            Request::build("workspace/didChangeWatchedFiles")
                .params(json!({"changes": [{"uri": old_uri, "type": FileChangeType::CREATED}]}))
                .finish(),
        )
        .await;
    assert_eq!(wait_for_refresh(&mut server).await, 0);

    std::fs::rename(&old_path, &definitions).unwrap();
    server
        .send_request(
            Request::build("workspace/didRenameFiles")
                .params(json!({"files": [{"oldUri": old_uri, "newUri": definitions_uri}]}))
                .finish(),
        )
        .await;
    assert_eq!(wait_for_refresh(&mut server).await, 0);
    assert_eq!(
        semantic_lines(&mut server, &references_uri, references, variable).await,
        [0]
    );
    server
        .send_request(
            Request::build("textDocument/definition")
                .params(json!({
                    "textDocument": {"uri": references_uri},
                    "position": {"line": 0, "character": references.find("WIDTH").unwrap()},
                }))
                .id(2)
                .finish(),
        )
        .await;
    let response = server.recv_response_skipping().await;
    assert_eq!(response["result"]["uri"], json!(definitions_uri));
}

async fn wait_for_refresh(server: &mut TestServer) -> usize {
    let mut files_analyzed = 0;
    loop {
        let request = server.recv_notification().await;
        if request.method() == "workspace/semanticTokens/refresh" {
            server.send_ack(request.id().unwrap()).await;
            return files_analyzed;
        }
        if request.method() == "window/logMessage"
            && request
                .params()
                .and_then(|params| params["message"].as_str())
                .is_some_and(|message| message.starts_with("background_analyze: "))
        {
            files_analyzed += 1;
        }
    }
}

async fn semantic_lines(
    server: &mut TestServer,
    uri: &Url,
    references: &str,
    variable: u32,
) -> Vec<u32> {
    server
        .send_request(
            Request::build("textDocument/semanticTokens/full")
                .params(json!({"textDocument": {"uri": uri}}))
                .id(22)
                .finish(),
        )
        .await;
    let response = server.recv_response_skipping().await;
    let tokens: SemanticTokens = serde_json::from_value(response["result"].clone()).unwrap();
    let mut line = 0;
    let mut column = 0;
    let mut highlighted = Vec::new();
    let lines: Vec<_> = references.lines().collect();
    for token in tokens.data {
        line += token.delta_line;
        column = if token.delta_line == 0 {
            column + token.delta_start
        } else {
            token.delta_start
        };
        let source = lines[line as usize];
        let text = &source[column as usize..(column + token.length) as usize];
        if token.token_type == variable && text == "WIDTH" {
            highlighted.push(line);
        }
    }
    highlighted
}

async fn check_semantic_tokens_after_declaration_edit(
    definitions: &str,
    without_width: &str,
    references: &str,
    expected_lines: &[u32],
    navigation: (Position, u32),
    disk_source: Option<&str>,
) {
    let project = tempfile::tempdir().unwrap();
    let root = editor_project_root(&project);
    std::fs::write(
        root.join("Veryl.toml"),
        "[project]\nname = \"highlight_test\"\nversion = \"0.1.0\"\n[build]\nexclude_std = true\nincremental = false\n",
    )
    .unwrap();
    let definitions_file = root.join("definitions.veryl");
    let references_file = root.join("references.veryl");
    std::fs::write(&definitions_file, definitions).unwrap();
    std::fs::write(&references_file, references).unwrap();
    let definitions_uri = Url::from_file_path(definitions_file).unwrap();
    let references_uri = Url::from_file_path(references_file).unwrap();
    let disk_uri = if let Some(disk_source) = disk_source {
        let file = root.join("disk.veryl");
        std::fs::write(&file, disk_source).unwrap();
        Some(Url::from_file_path(file).unwrap())
    } else {
        None
    };

    let mut server = TestServer::new(Backend::new);
    server
        .send_request(
            Request::build("initialize")
                .params(json!({"capabilities": {"workspace": {"semanticTokens": {"refreshSupport": true}}}}))
                .id(1)
                .finish(),
        )
        .await;
    let initialize = server.recv_response_skipping().await;
    let variable =
        initialize["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|x| x == "variable")
            .unwrap() as u32;
    server.send_request(build_initialized()).await;

    let edited_definitions = format!("{definitions}\n\n");
    let edited_references = format!("{references}\n");
    let updates = [
        (&references_uri, 1, references, expected_lines),
        (&definitions_uri, 1, definitions, expected_lines),
        (
            &definitions_uri,
            2,
            edited_definitions.as_str(),
            expected_lines,
        ),
        (&definitions_uri, 3, without_width, &[]),
        (&definitions_uri, 4, "package", &[]),
        (&definitions_uri, 5, definitions, expected_lines),
        (
            &references_uri,
            2,
            edited_references.as_str(),
            expected_lines,
        ),
    ];
    for (step, (uri, version, text, expected_lines)) in updates.into_iter().enumerate() {
        let method = if version == 1 {
            "textDocument/didOpen"
        } else {
            "textDocument/didChange"
        };
        let params = if method == "textDocument/didOpen" {
            json!({"textDocument": {"uri": uri, "languageId": "veryl", "version": version, "text": text}})
        } else {
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]})
        };
        server
            .send_request(Request::build(method).params(params).finish())
            .await;
        let files_analyzed = wait_for_refresh(&mut server).await;
        if step == 0 {
            assert!(files_analyzed > 0, "the first open should load the project");
        } else {
            assert_eq!(files_analyzed, 0, "the loaded project should be reused");
        }
        if step % 2 == 1 {
            assert_eq!(
                semantic_lines(&mut server, &references_uri, references, variable).await,
                expected_lines
            );
        }

        let (position, definition_line) = navigation;
        let params = json!({
            "textDocument": {"uri": references_uri},
            "position": position,
            "context": {"includeDeclaration": false},
        });
        server
            .send_request(
                Request::build("textDocument/definition")
                    .params(params.clone())
                    .id(20)
                    .finish(),
            )
            .await;
        let response = server.recv_response_skipping().await;
        let definition: Option<Location> =
            serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(
            definition
                .as_ref()
                .map(|location| (&location.uri, location.range.start.line)),
            (!expected_lines.is_empty()).then_some((&definitions_uri, definition_line)),
        );
        server
            .send_request(
                Request::build("textDocument/references")
                    .params(params)
                    .id(21)
                    .finish(),
            )
            .await;
        let response = server.recv_response_skipping().await;
        let locations: Vec<Location> = serde_json::from_value(response["result"].clone()).unwrap();
        let mut open_lines: Vec<_> = locations
            .iter()
            .filter(|location| location.uri == references_uri)
            .map(|location| location.range.start.line)
            .collect();
        open_lines.sort_unstable();
        assert_eq!(open_lines, expected_lines);
        if let Some(disk_uri) = &disk_uri {
            assert_eq!(
                locations.iter().any(|location| location.uri == *disk_uri),
                !expected_lines.is_empty()
            );
        }
        if step % 2 == 0 {
            assert_eq!(
                semantic_lines(&mut server, &references_uri, references, variable).await,
                expected_lines
            );
        }
    }
}

fn editor_project_root(project: &tempfile::TempDir) -> PathBuf {
    #[cfg(unix)]
    {
        let real = project.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = project.path().join("editor");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        alias
    }
    #[cfg(not(unix))]
    project.path().to_path_buf()
}

fn build_did_open(text: &str) -> Request {
    let mut path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    path.pop();
    path.pop();
    path.push("test.veryl");
    build_did_open_at(Url::from_file_path(path).unwrap(), text)
}

fn build_did_open_at(uri: Url, text: &str) -> Request {
    let text_document = TextDocumentItem {
        uri,
        language_id: "veryl".to_string(),
        version: 0,
        text: text.to_string(),
    };

    let params = DidOpenTextDocumentParams { text_document };

    Request::build("textDocument/didOpen")
        .params(json!(params))
        .finish()
}

fn completion_project() -> (tempfile::TempDir, Url) {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Veryl.toml"),
        "[project]\nname = \"completion_test\"\nversion = \"0.1.0\"\n[build]\nexclude_std = true\nincremental = false\n",
    )
    .unwrap();
    let uri = Url::from_file_path(project.path().join("test.veryl")).unwrap();
    (project, uri)
}

fn build_completion(
    uri: Url,
    id: i64,
    line: u32,
    character: u32,
    context: CompletionContext,
) -> Request {
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position { line, character },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: Some(context),
    };

    Request::build("textDocument/completion")
        .params(json!(params))
        .id(id)
        .finish()
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn completion_tb_component_method() {
    let (_project, uri) = completion_project();
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let res = server.recv_response().await;
    assert!(res.is_ok());

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");

    // `clk.` and `next();` are split across lines so the line under the
    // cursor ends with the member access under completion while the whole
    // document still parses.
    let code = "#[test(t)]\nmodule t {\n    inst clk: $tb::clock_gen;\n    initial {\n        clk.\n        next();\n    }\n}\n";
    let req = build_did_open_at(uri.clone(), code);
    server.send_request(req).await;

    // Cursor right after `clk.` (0-based position).
    let req = build_completion(
        uri,
        2,
        4,
        12,
        CompletionContext {
            trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some(".".to_string()),
        },
    );
    server.send_request(req).await;

    let res = server.recv_response_skipping().await;
    let items = res["result"].as_array().unwrap();
    assert!(
        items.iter().any(|x| x["label"] == "next"),
        "expected `next` in completion items, got: {items:?}",
    );
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn completion_in_unparsable_document() {
    let (_project, uri) = completion_project();
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let res = server.recv_response().await;
    assert!(res.is_ok());

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");

    // The only open source does not parse, so nothing of the project has been analyzed.
    let code = "module t {\n    let a: logic = mo\n}\n";
    let req = build_did_open_at(uri.clone(), code);
    server.send_request(req).await;

    let context = CompletionContext {
        trigger_kind: CompletionTriggerKind::INVOKED,
        trigger_character: None,
    };
    let req = build_completion(uri, 2, 1, 21, context);
    server.send_request(req).await;

    let res = server.recv_response_skipping().await;
    let items = res["result"].as_array().unwrap();
    assert!(
        items.iter().any(|x| x["label"] == "module"),
        "expected keyword items, got: {items:?}",
    );
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn did_open() {
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let res = server.recv_response().await;
    assert!(res.is_ok());

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "server initialized!");

    let req = build_did_open("module A {}");
    server.send_request(req).await;

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "did_open");

    let res = server.recv_notification().await;
    dbg!(&res);
    assert_eq!(res.method(), "textDocument/publishDiagnostics");
    assert!(
        res.params().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn diagnostics() {
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let res = server.recv_response().await;
    assert!(res.is_ok());

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "server initialized!");

    let req = build_did_open("module A  var a: logic; }");
    server.send_request(req).await;

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "did_open");

    let res = server.recv_notification().await;
    dbg!(&res);
    assert_eq!(res.method(), "textDocument/publishDiagnostics");
    let diags = res.params().unwrap()["diagnostics"].as_array().unwrap();
    assert_eq!(diags[0]["code"], Value::from("ParserError::SyntaxError"));
    assert_eq!(diags[0]["range"]["start"]["character"], Value::from(10));
    assert_eq!(diags[0]["range"]["start"]["line"], Value::from(0));
    assert_eq!(diags[0]["range"]["end"]["character"], Value::from(13));
    assert_eq!(diags[0]["range"]["end"]["line"], Value::from(0));
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn combinational_loop_diagnostic() {
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let _ = server.recv_response().await;

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");

    let code = "module Top (\n    a: input  logic<8>,\n    b: output logic<8>,\n    c: output logic<8>,\n) {\n    assign b = c + a;\n    assign c = b + 1;\n}\n";
    let req = build_did_open(code);
    server.send_request(req).await;

    // did_open log
    let _ = server.recv_notification().await;

    // diagnostics
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "textDocument/publishDiagnostics");
    let diags = res.params().unwrap()["diagnostics"].as_array().unwrap();
    assert!(!diags.is_empty(), "expected combinational_loop diagnostic");
    let diag = &diags[0];
    assert_eq!(diag["code"], Value::from("combinational_loop"));
    assert_eq!(
        diag["message"],
        Value::from("Semantic Error: combinational loop detected: b -> c -> b"),
    );
    let related = diag["relatedInformation"].as_array();
    assert!(
        related.is_some_and(|r| !r.is_empty()),
        "expected non-empty relatedInformation, got: {diag}",
    );
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn instantiation_note_diagnostic() {
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let _ = server.recv_response().await;

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");

    let code = "module Leaf #(param W: u32 = 1) (i: input logic<8>, o: output logic) {\n    assign o = i[W + 5];\n}\nmodule Top (i: input logic<8>, o: output logic) {\n    inst l: Leaf #(W: 9) (i, o);\n}\n";
    let req = build_did_open(code);
    server.send_request(req).await;

    // did_open log
    let _ = server.recv_notification().await;

    // diagnostics
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "textDocument/publishDiagnostics");
    let diags = res.params().unwrap()["diagnostics"].as_array().unwrap();
    let diag = diags
        .iter()
        .find(|x| x["code"] == "invalid_select")
        .unwrap_or_else(|| panic!("expected invalid_select, got: {diags:?}"));
    let related = diag["relatedInformation"].as_array().unwrap();
    assert_eq!(related.len(), 1, "{diag}");
    assert_eq!(
        related[0]["message"],
        Value::from("reported inside l: Leaf #(W: 9)")
    );
    assert_eq!(
        related[0]["location"]["range"]["start"]["line"],
        Value::from(4)
    );
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn progress() {
    let mut server = TestServer::new(Backend::new);

    let req = build_initialize(1);
    server.send_request(req).await;
    let res = server.recv_response().await;
    assert!(res.is_ok());

    let req = build_initialized();
    server.send_request(req).await;
    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "server initialized!");

    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Veryl.toml"),
        "[project]\nname=\"progress_test\"\nversion=\"0.1.0\"\n[build]\nexclude_std=true\n",
    )
    .unwrap();
    std::fs::write(project.path().join("child.veryl"), "module Child {}").unwrap();
    let uri = Url::from_file_path(project.path().join("test.veryl")).unwrap();
    let req = Request::build("textDocument/didOpen").params(json!({"textDocument":{"uri":uri,"languageId":"veryl","version":0,"text":"module A {}"}})).finish();
    server.send_request(req).await;

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "did_open");

    let res = server.recv_notification().await;
    dbg!(&res);
    assert_eq!(res.method(), "textDocument/publishDiagnostics");

    let mut res = server.recv_notification().await;
    while res.method() == "textDocument/publishDiagnostics" {
        res = server.recv_notification().await;
    }
    assert_eq!(res.method(), "window/workDoneProgress/create");

    server.send_ack(res.id().unwrap()).await;

    let mut progress_finish = false;
    let mut percentage = 0;
    while !progress_finish {
        let res = server.recv_notification().await;
        dbg!(&res);
        if res.method() == "$/progress" {
            if res.params().unwrap()["value"]["kind"] == "end" {
                progress_finish = true;
            } else if res.params().unwrap()["value"]["kind"] == "report" {
                percentage = res.params().unwrap()["value"]["percentage"]
                    .as_i64()
                    .unwrap();
            }
        }
    }
    assert_eq!(percentage, 100);
}
