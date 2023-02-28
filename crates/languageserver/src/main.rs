#![recursion_limit = "256"]

use tower_lsp::{LspService, Server};

mod backend;
mod keyword;
mod server;
use backend::Backend;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
