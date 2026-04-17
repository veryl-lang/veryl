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

fn build_did_open(text: &str) -> Request {
    let mut path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    path.pop();
    path.pop();
    path.push("test.veryl");
    let uri = Url::from_file_path(path).unwrap();
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

    let req = build_did_open("module A {}");
    server.send_request(req).await;

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "did_open");

    let res = server.recv_notification().await;
    dbg!(&res);
    assert_eq!(res.method(), "textDocument/publishDiagnostics");

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/workDoneProgress/create");

    server.send_ack(res.id().unwrap()).await;

    let mut progress_finish = false;
    let mut percentage = 0;
    while !progress_finish {
        let res = server.recv_notification().await;
        dbg!(&res);
        if res.method() == "$/progress" {
            if res.params().unwrap()["value"]["kind"] == Value::from("end") {
                progress_finish = true;
            } else if res.params().unwrap()["value"]["kind"] == Value::from("report") {
                percentage = res.params().unwrap()["value"]["percentage"]
                    .as_i64()
                    .unwrap();
            }
        }
    }
    assert_eq!(percentage, 100);
}
