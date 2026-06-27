#![recursion_limit = "256"]

mod backend;
mod incremental;
mod keyword;
mod server;
#[cfg(test)]
mod tests;

use backend::Backend;
use tower_lsp_server::{LspService, Server};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
