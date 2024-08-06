use crate::Backend;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tower_lsp::jsonrpc::{Id, Request, Response};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct TestServer {
    req_stream: DuplexStream,
    res_stream: DuplexStream,
    responses: VecDeque<String>,
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
            responses: VecDeque::new(),
        }
    }

    fn encode(payload: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload)
    }

    fn decode(text: &str) -> Vec<String> {
        let mut ret = Vec::new();
        let mut temp = text;

        while !temp.is_empty() {
            let p = temp.find("\r\n\r\n").unwrap();
            let (header, body) = temp.split_at(p + 4);
            let len = header
                .strip_prefix("Content-Length: ")
                .unwrap()
                .strip_suffix("\r\n\r\n")
                .unwrap();
            let len: usize = len.parse().unwrap();
            let (body, rest) = body.split_at(len);
            ret.push(body.to_string());
            temp = rest;
        }

        ret
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

    async fn recv_response(&mut self) -> Response {
        if self.responses.is_empty() {
            let mut buf = vec![0; 1024];
            let n = self.res_stream.read(&mut buf).await.unwrap();
            let ret = String::from_utf8(buf[..n].to_vec()).unwrap();
            for x in Self::decode(&ret) {
                self.responses.push_front(x);
            }
        }
        let res = self.responses.pop_back().unwrap();
        serde_json::from_str(&res).unwrap()
    }

    async fn recv_notification(&mut self) -> Request {
        if self.responses.is_empty() {
            let mut buf = vec![0; 1024];
            let n = self.res_stream.read(&mut buf).await.unwrap();
            let ret = String::from_utf8(buf[..n].to_vec()).unwrap();
            for x in Self::decode(&ret) {
                self.responses.push_front(x);
            }
        }
        let res = self.responses.pop_back().unwrap();
        serde_json::from_str(&res).unwrap()
    }
}

fn build_initialize(id: i64) -> Request {
    let params = InitializeParams::default();
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
    assert!(res.params().unwrap()["diagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
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

    let req = build_did_open("module A { var a: logic; }");
    server.send_request(req).await;

    let res = server.recv_notification().await;
    assert_eq!(res.method(), "window/logMessage");
    assert_eq!(res.params().unwrap()["message"], "did_open");

    let res = server.recv_notification().await;
    dbg!(&res);
    assert_eq!(res.method(), "textDocument/publishDiagnostics");
    let diags = res.params().unwrap()["diagnostics"].as_array().unwrap();
    assert_eq!(diags[0]["code"], Value::from("unused_variable"));
    assert_eq!(diags[0]["range"]["start"]["character"], Value::from(15));
    assert_eq!(diags[0]["range"]["start"]["line"], Value::from(0));
    assert_eq!(diags[0]["range"]["end"]["character"], Value::from(16));
    assert_eq!(diags[0]["range"]["end"]["line"], Value::from(0));
}

#[tokio::test]
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
