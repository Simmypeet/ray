//! Drives an in-process language server through the LSP wire protocol, for
//! the suites whose fixtures ask the server something at a cursor.
//!
//! Each fixture is the root file of a project, with one cursor marker. The
//! fixture is opened as the only document of a fresh project, and the request
//! is sent at the marker once the project has been checked. The project is in
//! a temporary directory, which the response refers to as `file:///project/`.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
    },
    task::JoinHandle,
};
use tower_lsp::{Server, lsp_types::Url};

/// Marks where a fixture asks the server. It is removed from the document the
/// server sees.
const CURSOR: &str = "<cursor>";

/// The URI that stands for the directory of the project in a response.
const PROJECT_URI: &str = "file:///project/";

/// How long the server may take to answer before the test fails.
const TIMEOUT: Duration = Duration::from_secs(120);

/// A `main.ray` fixture with a cursor marker.
#[derive(Debug)]
pub struct Fixture {
    path: PathBuf,
}

impl Fixture {
    /// Locates the fixture at `resource`, a path relative to the workspace.
    #[must_use]
    pub fn new(resource: &str) -> Self {
        let workspace = dunce::canonicalize(env!("RAYC_CARGO_WORKSPACE_DIR")).unwrap();
        Self { path: workspace.join(resource) }
    }

    /// Returns the settings that store the snapshot of the fixture next to
    /// it, as `snapshot.snap`.
    #[must_use]
    pub fn snapshot_settings(&self) -> insta::Settings {
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(self.path.parent().unwrap());
        settings.set_prepend_module_to_snapshot(false);
        settings.remove_snapshot_suffix();
        settings
    }

    /// Sends the request `method` at the cursor of the fixture, with the
    /// parameters of a position in a document, and returns the response.
    #[must_use]
    pub fn request_at_cursor(&self, method: &str) -> Value {
        stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(if cfg!(debug_assertions) {
                    8 * 1024 * 1024
                } else {
                    2 * 1024 * 1024
                })
                .build()
                .expect("failed to create Tokio runtime")
                .block_on(request(&self.path, method))
        })
    }
}

async fn request(fixture: &Path, method: &str) -> Value {
    let text = std::fs::read_to_string(fixture).unwrap();
    let (source, position) = split_cursor(&text);

    // the server works only in project mode, so the fixture is the root file
    // of a fresh project
    let project = tempfile::tempdir().unwrap();
    let directory = dunce::canonicalize(project.path()).unwrap();
    std::fs::write(directory.join("ray.toml"), "[target]\nname = \"app\"\n").unwrap();
    std::fs::create_dir(directory.join("src")).unwrap();
    let root_file = directory.join("src").join("main.ray");
    std::fs::write(&root_file, &source).unwrap();
    let uri = Url::from_file_path(&root_file).unwrap();

    // open the document and wait for its first check, so that its project is
    // loaded before the request
    let mut editor = Editor::start();
    editor.request("initialize", &json!({ "capabilities": {} })).await;
    editor.notify("initialized", &json!({})).await;
    editor
        .notify(
            "textDocument/didOpen",
            &json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "ray",
                    "version": 1,
                    "text": source,
                },
            }),
        )
        .await;
    editor.wait_for_diagnostics(&uri).await;

    let response = editor
        .request(method, &json!({ "textDocument": { "uri": uri }, "position": position }))
        .await;
    editor.stop().await;

    // the directory changes from run to run
    let directory_uri = Url::from_directory_path(&directory).unwrap();
    serde_json::from_str(&response.to_string().replace(directory_uri.as_str(), PROJECT_URI))
        .unwrap()
}

/// Removes the cursor marker from `text`, returning the text without it and
/// the LSP position of the marker.
fn split_cursor(text: &str) -> (String, Value) {
    let (before, after) = text
        .split_once(CURSOR)
        .unwrap_or_else(|| panic!("a fixture marks its cursor with `{CURSOR}`"));
    assert!(!after.contains(CURSOR), "a fixture has a single cursor");

    // LSP columns count UTF-16 code units
    let line = before.matches('\n').count();
    let character = before.rsplit('\n').next().unwrap_or_default().encode_utf16().count();

    (format!("{before}{after}"), json!({ "line": line, "character": character }))
}

/// An editor talking to an in-process language server through the LSP wire
/// protocol.
struct Editor {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    server: JoinHandle<()>,
    next_id: i64,
}

impl Editor {
    fn start() -> Self {
        let (editor, server) = tokio::io::duplex(1 << 16);
        let (server_reader, server_writer) = tokio::io::split(server);
        let (service, socket) = ray_lsp::Backend::service();
        let server = tokio::spawn(Server::new(server_reader, server_writer, socket).serve(service));

        let (reader, writer) = tokio::io::split(editor);
        Self { reader: BufReader::new(reader), writer, server, next_id: 1 }
    }

    async fn send(&mut self, message: &Value) {
        let body = message.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        self.writer.write_all(frame.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn receive(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).await.unwrap();
            assert_ne!(read, 0, "the server stopped before answering");
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }

        let mut body = vec![0; length.expect("a message states its length")];
        self.reader.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params })).await;
    }

    /// Sends a request and returns its response, skipping the notifications
    /// sent meanwhile.
    async fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;

        self.receive_until(|message| message.get("method").is_none() && message["id"] == id).await
    }

    async fn wait_for_diagnostics(&mut self, uri: &Url) {
        self.receive_until(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri.as_str()
        })
        .await;
    }

    async fn receive_until(&mut self, mut is_awaited: impl FnMut(&Value) -> bool) -> Value {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let message = self.receive().await;
                if is_awaited(&message) {
                    return message;
                }
            }
        })
        .await
        .expect("the server answers in time")
    }

    /// Shuts the server down, which releases the incremental cache in the
    /// project directory before it is deleted.
    async fn stop(mut self) {
        self.request("shutdown", &Value::Null).await;
        self.notify("exit", &Value::Null).await;

        // the server stops reading once its input closes, as when an editor
        // exits; the stream closes only when both of its halves are dropped
        let Self { reader, writer, mut server, .. } = self;
        drop(reader);
        drop(writer);

        if tokio::time::timeout(TIMEOUT, &mut server).await.is_err() {
            server.abort();
            panic!("the server did not stop after its input closed");
        }
    }
}
