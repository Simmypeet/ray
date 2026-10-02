//! Stdio entry point for the Ray language server.

use tower_lsp::{LspService, Server};

fn main() {
    // Compiler futures need the same larger debug stack as the CLI.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(if cfg!(debug_assertions) { 8 * 1024 * 1024 } else { 2 * 1024 * 1024 })
        .build()
        .unwrap()
        .block_on(async {
            // `block_on` polls on the main thread, whose stack is only 1MB on
            // Windows; run the server on a worker with the stack size above.
            tokio::spawn(async {
                let (service, socket) = LspService::new(ray_lsp::Backend::new);
                Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
            })
            .await
            .unwrap();
        });
}
