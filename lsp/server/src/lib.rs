//! Ray language server: live diagnostics for each open source file.

use std::collections::HashMap;

use compiler::Compiler;
use tokio::sync::Mutex;
use tower_lsp::{
    Client, LanguageServer,
    jsonrpc::Result,
    lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        InitializeParams, InitializeResult, MessageType, ServerCapabilities, ServerInfo,
        TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    },
};

mod compiler;
mod diagnostic;

#[derive(Debug)]
struct Document {
    compiler: Compiler,
    version: i32,
}

/// A stdio-compatible language server backed by the Ray check pipeline.
#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: Mutex<HashMap<Url, Document>>,
}

impl Backend {
    /// Creates a language server connected to the given LSP client.
    #[must_use]
    pub fn new(client: Client) -> Self { Self { client, documents: Mutex::new(HashMap::new()) } }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "ray_lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            ..InitializeResult::default()
        })
    }

    async fn shutdown(&self) -> Result<()> { Ok(()) }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let Ok(path) = document.uri.to_file_path() else {
            self.client
                .log_message(MessageType::WARNING, "Ray diagnostics require a file URI")
                .await;
            return;
        };

        // Serialize document updates and publication so old diagnostics cannot
        // overwrite newer results or reappear after the document is closed.
        let mut documents = self.documents.lock().await;
        let compiler = Compiler::new(path).await;
        let diagnostics = compiler.check(&document.uri, &document.text).await;
        self.client
            .publish_diagnostics(document.uri.clone(), diagnostics, Some(document.version))
            .await;
        documents.insert(document.uri, Document { compiler, version: document.version });
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let mut documents = self.documents.lock().await;
        let uri = params.text_document.uri;
        let Some(document) = documents.get_mut(&uri) else {
            return;
        };
        if params.text_document.version <= document.version {
            return;
        }
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        if change.range.is_some() {
            self.client
                .log_message(MessageType::ERROR, "Ray requires full document synchronization")
                .await;
            return;
        }

        let diagnostics = document.compiler.check(&uri, &change.text).await;
        document.version = params.text_document.version;
        self.client.publish_diagnostics(uri, diagnostics, Some(document.version)).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut documents = self.documents.lock().await;
        documents.remove(&params.text_document.uri);
        self.client.publish_diagnostics(params.text_document.uri, Vec::new(), None).await;
    }
}
