//! Exercises editor-visible diagnostics through the server's stdio protocol.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tower_lsp::lsp_types::Url;

const VALID_MAIN: &str = "def main() -> int32:\n    return 0\n";

struct Editor {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
}

impl Editor {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ray_lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap() == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length: ") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; length.unwrap()];
                stdout.read_exact(&mut body).unwrap();
                if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut editor = Self { child, stdin: Some(stdin), messages, next_id: 1 };
        let response = editor.request("initialize", &json!({"capabilities":{}}));
        // incremental synchronization
        assert_eq!(response["result"]["capabilities"]["textDocumentSync"], 2);
        editor.notify("initialized", &json!({}));
        editor
    }

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        let stdin = self.stdin.as_mut().expect("the server is running");
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc":"2.0", "method":method, "params":params}));
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"jsonrpc":"2.0", "id":id, "method":method});

        // a request without parameters leaves them out, as editors do
        if !params.is_null() {
            request["params"] = params.clone();
        }
        self.send(&request);

        loop {
            let message = self.receive();
            if message["id"] == id {
                return message;
            }
            assert_ne!(message["method"], "textDocument/publishDiagnostics", "{message}");
        }
    }

    fn open(&mut self, uri: &Url, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            &json!({"textDocument":{
                "uri":uri, "languageId":"ray", "version":version, "text":text,
            }}),
        );
    }

    fn change(&mut self, uri: &Url, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            &json!({
                "textDocument":{"uri":uri, "version":version}, "contentChanges":[{"text":text}],
            }),
        );
    }

    fn receive(&self) -> Value {
        self.messages.recv_timeout(Duration::from_secs(60)).expect("server response")
    }

    /// Returns the next diagnostics published for `uri`, which must be for
    /// `version`.
    ///
    /// Diagnostics of the other files of the project are skipped. Any message
    /// shown to the user fails the test.
    fn diagnostics(&self, uri: &Url, version: Option<i32>) -> Vec<Value> {
        loop {
            let message = self.receive();
            assert_ne!(message["method"], "window/showMessage", "{message}");
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri.as_str()
            {
                assert_eq!(message["params"]["version"], json!(version));
                return message["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    /// Returns the diagnostics published by one check, which must cover
    /// exactly `count` documents, keyed by URI with their version.
    fn publications(&self, count: usize) -> HashMap<String, (Value, Vec<Value>)> {
        let mut publications = HashMap::new();
        while publications.len() < count {
            let message = self.receive();
            assert_ne!(message["method"], "window/showMessage", "{message}");
            if message["method"] == "textDocument/publishDiagnostics" {
                let params = &message["params"];
                let diagnostics = params["diagnostics"].as_array().unwrap().clone();
                let previous = publications.insert(
                    params["uri"].as_str().unwrap().to_owned(),
                    (params["version"].clone(), diagnostics),
                );
                assert!(previous.is_none(), "published twice: {message}");
            }
        }
        publications
    }

    /// Returns the next message shown to the user.
    fn shown_message(&self) -> Value {
        loop {
            let message = self.receive();
            assert_ne!(message["method"], "textDocument/publishDiagnostics", "{message}");
            if message["method"] == "window/showMessage" {
                return message["params"].clone();
            }
        }
    }

    /// Shuts the server down and waits for it to exit.
    fn shut_down(mut self) {
        self.request("shutdown", &json!(null));
        self.notify("exit", &json!(null));

        // the server stops reading once its input closes, as when an editor
        // exits
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Creates a project named `app` containing `files`.
fn project(files: &[(&str, &str)]) -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(&directory.path().join("ray.toml"), "[target]\nname = \"app\"\n");
    for (path, contents) in files {
        write(&directory.path().join(path), contents);
    }
    directory
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn uri(directory: &TempDir, path: &str) -> Url {
    Url::from_file_path(directory.path().join(path)).unwrap()
}

#[test]
fn diagnostics_follow_unsaved_edits_and_close() {
    // Editors must see compiler errors from the buffer rather than the saved
    // file, and see those errors disappear after a correction.
    let directory = project(&[("src/main.ray", VALID_MAIN)]);
    let path = directory.path().join("src/main.ray");
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    let invalid = "def missingReturn() -> int32:\n    let value = 42\n";
    editor.open(&uri, 1, invalid);
    let diagnostics = editor.diagnostics(&uri, Some(1));
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().all(|diagnostic| diagnostic["source"] == "ray"));
    assert_eq!(
        diagnostics[0]["range"],
        json!({
            "start": {"line":0, "character":4},
            "end": {"line":0, "character":17},
        })
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["message"].as_str().unwrap().contains("return"))
    );

    editor.change(&uri, 2, "def missingReturn() -> int32:\n    return 42\n");
    assert!(editor.diagnostics(&uri, Some(2)).is_empty());

    editor.change(&uri, 3, invalid);
    assert_eq!(editor.diagnostics(&uri, Some(3)), diagnostics);
    editor.change(&uri, 4, "def");
    assert!(!editor.diagnostics(&uri, Some(4)).is_empty());
    // Reintroducing a different declaration must also recover in the same engine.
    editor.change(&uri, 5, "def replacement() -> int32:\n    return 42\n");
    assert!(editor.diagnostics(&uri, Some(5)).is_empty());

    // Closing the document checks the saved file again.
    editor.notify("textDocument/didClose", &json!({"textDocument":{"uri":uri}}));
    assert!(editor.diagnostics(&uri, None).is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID_MAIN);

    editor.shut_down();
}

#[test]
fn ranged_edits_use_utf16_positions() {
    // An editor sends only the edited ranges, in UTF-16 columns, and several
    // edits of one change apply in order.
    let text = "def main() -> int32:\n    let value = (\"😀\", 1)\n    return missing\n";
    let directory = project(&[("src/main.ray", text)]);
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, text);
    assert!(!editor.diagnostics(&uri, Some(1)).is_empty());

    // the second edit lands inside the string only if the emoji counts as two
    // UTF-16 code units; counted as one character, it would break the syntax
    editor.notify(
        "textDocument/didChange",
        &json!({
            "textDocument":{"uri":uri, "version":2},
            "contentChanges":[
                {
                    "range":{"start":{"line":2, "character":11}, "end":{"line":2, "character":18}},
                    "text":"0",
                },
                {
                    "range":{"start":{"line":1, "character":20}, "end":{"line":1, "character":20}},
                    "text":"x",
                },
            ],
        }),
    );
    let diagnostics = editor.diagnostics(&uri, Some(2));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn diagnostic_columns_use_utf16_after_non_ascii_text() {
    // An editor must underline the unresolved name correctly even when an
    // earlier character occupies four UTF-8 bytes and two UTF-16 code units.
    let line = "    let value = (\"😀\", missing)";
    let text = format!("def main() -> ():\n{line}\n");
    let directory = project(&[("src/main.ray", &text)]);
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, &text);
    let diagnostics = editor.diagnostics(&uri, Some(1));
    let unresolved = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["message"].as_str().unwrap().contains("`missing`"))
        .expect("unresolved name diagnostic");
    let column = line[..line.find("missing").unwrap()].encode_utf16().count();
    assert_eq!(
        unresolved["range"],
        json!({
            "start":{"line":1, "character":column},
            "end":{"line":1, "character":column + "missing".len()},
        })
    );
}

/// Returns a valid instance of a core trait, and an invalid one whose method
/// does not match the trait declaration in the core library.
fn core_instance() -> (&'static str, String) {
    let valid =
        "inst Value for core.Def[int32]:\n    type Args = int32\n    type Return = int32\n    \
         type Effect = {}\n    def call(value: int32, a: int32) -> int32:\n        return value + \
         a\n";
    let invalid = valid
        .replace("call(value: int32", "call(value: bool")
        .replace("return value + a", "return a");
    (valid, invalid)
}

#[test]
fn core_diagnostics_follow_unsaved_edits() {
    let (valid, invalid) = core_instance();
    let directory = project(&[("src/main.ray", valid)]);
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, &invalid);
    let diagnostics = editor.diagnostics(&uri, Some(1));
    assert!(!diagnostics.is_empty());
    let core_locations: Vec<_> = diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic["relatedInformation"].as_array().into_iter().flatten())
        .filter(|related| related["location"]["uri"] == "ray-core:/core.ray")
        .collect();
    assert!(!core_locations.is_empty(), "{diagnostics:?}");
    assert!(
        core_locations.iter().any(|related| related["location"]["range"]["start"]["line"] == 5)
    );
    for (version, text) in [(2, valid), (3, invalid.as_str()), (4, valid)] {
        editor.change(&uri, version, text);
        let updated = editor.diagnostics(&uri, Some(version));
        if version == 3 {
            assert_eq!(updated, diagnostics);
        } else {
            assert!(updated.is_empty(), "{updated:?}");
        }
    }
}

#[test]
fn related_locations_are_distinct_and_core_ones_can_be_opened() {
    // The range of a diagnostic is not repeated as a related location, so its
    // label joins the message. A location in the bundled core library has no
    // file, so the editor asks the server for the core source.
    let (valid, invalid) = core_instance();
    let directory = project(&[("src/main.ray", valid)]);
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, &invalid);
    let diagnostics = editor.diagnostics(&uri, Some(1));
    let mismatch = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["message"].as_str().unwrap().contains("type mismatch"))
        .expect("parameter type mismatch diagnostic");
    assert!(
        mismatch["message"].as_str().unwrap().contains("expected `int32`, found `bool`"),
        "{mismatch}"
    );

    let related = mismatch["relatedInformation"].as_array().unwrap();
    assert!(
        related
            .iter()
            .all(|related| related["location"] != json!({"uri":uri, "range":mismatch["range"]})),
        "{mismatch}"
    );

    let core = related
        .iter()
        .find(|related| related["location"]["uri"] == "ray-core:/core.ray")
        .expect("core location");
    let source = editor.request("ray/coreSource", &json!(null));
    let line = core["location"]["range"]["start"]["line"].as_u64().unwrap();
    let declaration =
        source["result"].as_str().unwrap().lines().nth(usize::try_from(line).unwrap()).unwrap();
    assert!(declaration.contains("def call"), "{declaration}");
}

#[test]
fn files_outside_projects_suggest_creating_one() {
    // Only projects are checked, so the user is told how to create one.
    let directory = tempfile::tempdir().unwrap();
    let uri = uri(&directory, "main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, VALID_MAIN);
    let message = editor.shown_message();
    assert_eq!(message["type"], 2, "a warning: {message}");
    let text = message["message"].as_str().unwrap();
    assert!(text.contains("project mode") && text.contains("rayc new"), "{text}");
}

#[test]
fn invalid_manifests_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    write(&directory.path().join("ray.toml"), "[target]\nname = \"core\"\n");
    write(&directory.path().join("src/main.ray"), VALID_MAIN);
    let uri = uri(&directory, "src/main.ray");
    let mut editor = Editor::start();
    editor.open(&uri, 1, VALID_MAIN);
    let message = editor.shown_message();
    assert_eq!(message["type"], 1, "an error: {message}");
    assert!(message["message"].as_str().unwrap().contains("reserved"), "{message}");
}

#[test]
fn files_of_the_project_are_checked_from_disk() {
    // An error in a module file that is not open is published for that file,
    // and fixing the file on disk clears it.
    let directory = project(&[
        ("src/main.ray", "module util\n\ndef main() -> int32:\n    return util.value()\n"),
        ("src/util.ray", "pub def value() -> int32:\n    return true\n"),
    ]);
    let main = uri(&directory, "src/main.ray");
    // a file that is not open is published under its canonical path, which
    // differs on systems whose temporary directory is a symbolic link
    let util =
        Url::from_file_path(dunce::canonicalize(directory.path()).unwrap().join("src/util.ray"))
            .unwrap();
    let mut editor = Editor::start();
    editor.open(&main, 1, &std::fs::read_to_string(main.to_file_path().unwrap()).unwrap());

    let publications = editor.publications(2);
    assert_eq!(publications[main.as_str()], (json!(1), Vec::new()));
    let (version, diagnostics) = &publications[util.as_str()];
    assert_eq!(*version, Value::Null);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["range"]["start"], json!({"line":1, "character":11}));

    write(&util.to_file_path().unwrap(), "pub def value() -> int32:\n    return 1\n");
    editor.notify("workspace/didChangeWatchedFiles", &json!({"changes":[{"uri":util, "type":2}]}));
    let publications = editor.publications(2);
    assert_eq!(publications[main.as_str()], (json!(1), Vec::new()));
    assert_eq!(publications[util.as_str()], (Value::Null, Vec::new()));
}

#[test]
fn incremental_cache_does_not_keep_unsaved_edits() {
    // The cache outlives the server, but the unsaved contents of a document
    // must not: after a restart, the saved file is checked.
    let directory = project(&[("src/main.ray", VALID_MAIN)]);
    let uri = uri(&directory, "src/main.ray");

    let mut editor = Editor::start();
    editor.open(&uri, 1, "def main() -> int32:\n    return true\n");
    assert!(!editor.diagnostics(&uri, Some(1)).is_empty());
    editor.shut_down();
    assert!(directory.path().join("target/lsp").is_dir());

    // a cache that cannot be opened would be reported to the user
    let mut editor = Editor::start();
    editor.open(&uri, 1, VALID_MAIN);
    assert!(editor.diagnostics(&uri, Some(1)).is_empty());

    // closing the document without saving reads the same, saved file
    editor.change(&uri, 2, "def main() -> int32:\n    return true\n");
    assert!(!editor.diagnostics(&uri, Some(2)).is_empty());
    editor.notify("textDocument/didClose", &json!({"textDocument":{"uri":uri}}));
    assert!(editor.diagnostics(&uri, None).is_empty());
    editor.shut_down();

    let mut editor = Editor::start();
    editor.open(&uri, 1, VALID_MAIN);
    assert!(editor.diagnostics(&uri, Some(1)).is_empty());
}

#[test]
fn locked_incremental_cache_falls_back_to_memory() {
    // A second server for the same project cannot open the cache, but still
    // checks the project.
    let directory = project(&[("src/main.ray", "def main() -> int32:\n    return true\n")]);
    let uri = uri(&directory, "src/main.ray");
    let text = std::fs::read_to_string(uri.to_file_path().unwrap()).unwrap();

    let mut first = Editor::start();
    first.open(&uri, 1, &text);
    assert!(!first.diagnostics(&uri, Some(1)).is_empty());

    let mut second = Editor::start();
    second.open(&uri, 1, &text);
    let message = second.shown_message();
    assert_eq!(message["type"], 2, "a warning: {message}");
    assert!(message["message"].as_str().unwrap().contains("incremental cache"), "{message}");
    assert!(!second.diagnostics(&uri, Some(1)).is_empty());
}
