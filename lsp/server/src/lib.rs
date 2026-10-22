//! Ray language server: live diagnostics for the projects of open documents.
//!
//! Every open document is checked as part of the project containing it, which
//! is described by the nearest `ray.toml`. Each project has one engine that
//! caches its compilation in the project's output directory, so edits and
//! restarts recompute only what changed.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use overlay::Overlay;
use rayc_arena::Arena;
use rayc_project::{MANIFEST_FILE_NAME, Project};
use session::{Session, SessionID};
use tokio::sync::Mutex;
use tower_lsp::{
    Client, ClientSocket, LanguageServer, LspService,
    jsonrpc::Result,
    lsp_types::{
        DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, InitializeParams, InitializeResult, MessageType,
        ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
    },
};

mod diagnostic;
mod overlay;
mod session;

/// Messages to show to the user, each at most once.
#[derive(Debug, Default)]
struct Notices(Vec<(MessageType, String)>);

impl Notices {
    fn push(&mut self, kind: MessageType, message: String) {
        if !self.0.iter().any(|(_, existing)| *existing == message) {
            self.0.push((kind, message));
        }
    }

    async fn show(self, client: &Client) {
        for (kind, message) in self.0 {
            client.show_message(kind, message).await;
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// The projects being checked.
    sessions: Arena<Session>,

    /// The directories outside any project whose documents the user has been
    /// told about, so that it is told once per directory.
    reported_directories: HashSet<PathBuf>,
}

impl State {
    /// Returns the session checking the project in `directory`.
    fn session_id_of(&self, directory: &Path) -> Option<SessionID> {
        self.sessions
            .iter()
            .find(|(_, session)| session.directory() == directory)
            .map(|(session_id, _)| session_id)
    }

    /// Returns the session checking the innermost project containing `path`.
    fn innermost_session_id(&self, path: &Path) -> Option<SessionID> {
        self.sessions
            .iter()
            .filter(|(_, session)| path.starts_with(session.directory()))
            .max_by_key(|(_, session)| session.directory().as_os_str().len())
            .map(|(session_id, _)| session_id)
    }

    /// Schedules a check of the project of `session_id`, if it is still being
    /// checked.
    fn schedule_check(&mut self, session_id: SessionID, client: &Client, overlay: &Arc<Overlay>) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.schedule_check(client, overlay);
        }
    }
}

/// A stdio-compatible language server backed by the Ray check pipeline.
#[derive(Debug)]
pub struct Backend {
    client: Client,

    /// The open documents, which the compiler reads in place of their files.
    overlay: Arc<Overlay>,

    /// Serializes document updates and checks so that a check never misses a
    /// newer edit.
    state: Mutex<State>,
}

/// The request for the text of the bundled core library, which diagnostics
/// refer to by [`rayc_corelib::SOURCE_URI`].
///
/// The core library has no file, so an editor showing a core location asks the
/// server for it.
pub const CORE_SOURCE_REQUEST: &str = "ray/coreSource";

impl Backend {
    /// Creates a language server connected to the given LSP client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client, overlay: Arc::default(), state: Mutex::default() }
    }

    /// Creates the language service, including the requests beyond the
    /// protocol.
    #[must_use]
    pub fn service() -> (LspService<Self>, ClientSocket) {
        LspService::build(Self::new).custom_method(CORE_SOURCE_REQUEST, Self::core_source).finish()
    }

    /// Answers [`CORE_SOURCE_REQUEST`].
    #[allow(clippy::unused_async, clippy::unused_self)]
    async fn core_source(&self) -> Result<&'static str> { Ok(rayc_corelib::SOURCE) }

    /// Finds the project containing `path` and opens a session for it, or
    /// updates the session if the manifest changed.
    ///
    /// Returns the session of the project, or `None` if `path` is not in a
    /// valid project.
    async fn attach(
        &self,
        state: &mut State,
        path: &Path,
        notices: &mut Notices,
    ) -> Option<SessionID> {
        // a document that is not saved yet has no file to start from
        let directory = path.parent().unwrap_or(path);
        let project = match Project::discover(directory) {
            Ok(project) => project,

            // only projects are checked; tell the user how to create one
            Err(error) if error.is_manifest_not_found() => {
                if state.reported_directories.insert(directory.to_path_buf()) {
                    notices.push(
                        MessageType::WARNING,
                        format!(
                            "Ray language features work only in project mode, and '{}' is not in \
                             a project: no `{MANIFEST_FILE_NAME}` was found in its directory or \
                             any parent directory. Create a project with `rayc new <path>`.",
                            path.display()
                        ),
                    );
                }
                return None;
            }

            Err(error) => {
                notices.push(MessageType::ERROR, error.to_string());
                return None;
            }
        };

        if let Some(session_id) = state.session_id_of(project.directory()) {
            state.sessions[session_id].reload_if_changed(project).await;
            return Some(session_id);
        }

        let (session, warning) = Session::open(project, self.overlay.clone()).await;
        if let Some(warning) = warning {
            notices.push(MessageType::WARNING, warning);
        }

        Some(state.sessions.insert(session))
    }

    /// Re-reads the manifests after one of them changed, updating, opening or
    /// closing sessions as needed.
    async fn reload_projects(&self, state: &mut State, notices: &mut Notices) {
        // reload the projects being checked, closing those that no longer
        // have a valid manifest
        let session_ids = state.sessions.ids().collect::<Vec<_>>();
        for session_id in session_ids {
            if let Ok(project) = Project::load(state.sessions[session_id].directory()) {
                state.sessions[session_id].reload_if_changed(project).await;
            } else {
                let session = state.sessions.remove(session_id).expect("listed above");
                session.close(&self.client).await;
            }
        }

        // the open documents may now belong to another project, or to one for
        // the first time
        for buffer_id in self.overlay.buffer_ids() {
            let Some(source_path) = self.overlay.path(buffer_id) else {
                continue;
            };
            let session_id = self.attach(state, &source_path, notices).await;
            self.overlay.set_session(buffer_id, session_id);
        }

        for session in state.sessions.items_mut() {
            session.schedule_check(&self.client, &self.overlay);
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
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

    async fn shutdown(&self) -> Result<()> {
        // stop the checks and release the incremental caches; the client
        // discards the diagnostics itself
        let mut state = self.state.lock().await;
        for (_, session) in std::mem::take(&mut state.sessions) {
            session.stop().await;
        }

        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let Ok(path) = document.uri.to_file_path() else {
            self.client
                .log_message(MessageType::WARNING, "Ray diagnostics require a file URI")
                .await;
            return;
        };
        let source_path = Overlay::canonical_path(&path);

        let mut state = self.state.lock().await;
        let mut notices = Notices::default();
        let session_id = self.attach(&mut state, &source_path, &mut notices).await;

        // the document is opened before the check reads it
        self.overlay.open(document.uri, source_path, document.version, &document.text, session_id);

        if let Some(session_id) = session_id {
            state.schedule_check(session_id, &self.client, &self.overlay);
        }

        drop(state);

        notices.show(&self.client).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let mut state = self.state.lock().await;
        let document = params.text_document;
        if let Some(session_id) =
            self.overlay.edit_by_uri(&document.uri, document.version, params.content_changes)
        {
            state.schedule_check(session_id, &self.client, &self.overlay);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut state = self.state.lock().await;

        // the file on disk replaces the closed document, which may have had
        // unsaved edits
        if let Some(session_id) = self.overlay.close_by_uri(&params.text_document.uri) {
            state.schedule_check(session_id, &self.client, &self.overlay);
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut state = self.state.lock().await;
        let paths = params
            .changes
            .into_iter()
            .filter_map(|change| change.uri.to_file_path().ok())
            .map(|path| Overlay::canonical_path(&path))
            .collect::<Vec<_>>();

        // a manifest change can move documents between projects
        if paths.iter().any(|path| path.file_name().is_some_and(|name| name == MANIFEST_FILE_NAME))
        {
            let mut notices = Notices::default();
            self.reload_projects(&mut state, &mut notices).await;
            drop(state);

            notices.show(&self.client).await;
            return;
        }

        // re-check the projects whose source files changed on disk
        let session_ids = paths
            .iter()
            .filter_map(|path| state.innermost_session_id(path))
            .collect::<HashSet<_>>();
        for session_id in session_ids {
            state.schedule_check(session_id, &self.client, &self.overlay);
        }
    }
}
