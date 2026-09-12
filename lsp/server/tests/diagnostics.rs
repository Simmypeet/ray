//! Exercises editor-visible diagnostics through the server's stdio protocol.

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use serde_json::{Value, json};
use tower_lsp::lsp_types::Url;

struct Editor {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
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
        let mut editor = Self { child, stdin, messages };
        editor.send(
            &json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"capabilities":{}}}),
        );
        let response = editor.receive();
        assert_eq!(response["result"]["capabilities"]["textDocumentSync"], 1);
        editor.notify("initialized", &json!({}));
        editor
    }

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc":"2.0", "method":method, "params":params}));
    }

    fn receive(&self) -> Value {
        self.messages.recv_timeout(Duration::from_secs(60)).expect("server response")
    }

    fn diagnostics(&self, uri: &Url, version: Option<i32>) -> Vec<Value> {
        let response = self.receive();
        assert_eq!(response["method"], "textDocument/publishDiagnostics");
        assert_eq!(response["params"]["uri"], uri.as_str());
        assert_eq!(response["params"]["version"], json!(version));
        response["params"]["diagnostics"].as_array().unwrap().clone()
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn diagnostics_follow_unsaved_edits_and_close() {
    // Editors must see compiler errors from the buffer, including files that
    // have not been saved, and see those errors disappear after a correction.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.ray");
    let uri = Url::from_file_path(&path).unwrap();
    let mut editor = Editor::start();
    let invalid = "def missingReturn() -> int32:\n    let value = 42\n";
    editor.notify(
        "textDocument/didOpen",
        &json!({"textDocument":{
            "uri":uri, "languageId":"ray", "version":1, "text":invalid,
        }}),
    );
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

    editor.notify(
        "textDocument/didChange",
        &json!({
            "textDocument":{"uri":uri, "version":2},
            "contentChanges":[{"text":"def missingReturn() -> int32:\n    return 42\n"}],
        }),
    );
    assert!(editor.diagnostics(&uri, Some(2)).is_empty());

    editor.notify(
        "textDocument/didChange",
        &json!({
            "textDocument":{"uri":uri, "version":3}, "contentChanges":[{"text":invalid}],
        }),
    );
    assert_eq!(editor.diagnostics(&uri, Some(3)), diagnostics);
    editor.notify(
        "textDocument/didChange",
        &json!({
            "textDocument":{"uri":uri, "version":4}, "contentChanges":[{"text":"def"}],
        }),
    );
    assert!(!editor.diagnostics(&uri, Some(4)).is_empty());
    // Reintroducing a different declaration must also recover in the same engine.
    editor.notify(
        "textDocument/didChange",
        &json!({
            "textDocument":{"uri":uri, "version":5},
            "contentChanges":[{"text":"def replacement() -> int32:\n    return 42\n"}],
        }),
    );
    assert!(editor.diagnostics(&uri, Some(5)).is_empty());
    editor.notify("textDocument/didClose", &json!({"textDocument":{"uri":uri}}));
    assert!(editor.diagnostics(&uri, None).is_empty());
    assert!(!path.exists(), "checking must not save editor contents");

    editor.send(&json!({"jsonrpc":"2.0", "id":2, "method":"shutdown", "params":null}));
    assert_eq!(editor.receive()["id"], 2);
    editor.notify("exit", &json!(null));
}

#[test]
fn diagnostic_columns_use_utf16_after_non_ascii_text() {
    // An editor must underline the unresolved name correctly even when an
    // earlier character occupies four UTF-8 bytes and two UTF-16 code units.
    let directory = tempfile::tempdir().unwrap();
    let uri = Url::from_file_path(directory.path().join("unicode.ray")).unwrap();
    let line = "    let value = (\"😀\", missing)";
    let text = format!("def main() -> ():\n{line}\n");
    let mut editor = Editor::start();
    editor.notify(
        "textDocument/didOpen",
        &json!({"textDocument":{
            "uri":uri, "languageId":"ray", "version":1, "text":text,
        }}),
    );
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

#[test]
fn core_diagnostics_follow_unsaved_edits() {
    let directory = tempfile::tempdir().unwrap();
    let uri = Url::from_file_path(directory.path().join("unsaved.ray")).unwrap();
    let mut editor = Editor::start();
    let valid =
        "inst Value for core.Def[int32]:\n    type Args = int32\n    type Return = int32\n    \
         type Effect = {}\n    def call(value: int32, a: int32) -> int32:\n        return value + \
         a\n";
    let invalid = valid
        .replace("call(value: int32", "call(value: bool")
        .replace("return value + a", "return a");
    editor.notify(
        "textDocument/didOpen",
        &json!({"textDocument": {
            "uri": uri, "languageId":"ray", "version":1, "text":invalid,
        }}),
    );
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
        editor.notify(
            "textDocument/didChange",
            &json!({
                "textDocument":{"uri":uri,"version":version}, "contentChanges":[{"text":text}],
            }),
        );
        let updated = editor.diagnostics(&uri, Some(version));
        if version == 3 {
            assert_eq!(updated, diagnostics);
        } else {
            assert!(updated.is_empty(), "{updated:?}");
        }
    }
    let reserved_uri = Url::from_file_path(directory.path().join("core.ray")).unwrap();
    editor.notify(
        "textDocument/didOpen",
        &json!({"textDocument":{
            "uri":reserved_uri,"languageId":"ray","version":1,"text":valid,
        }}),
    );
    assert!(
        editor.diagnostics(&reserved_uri, Some(1))[0]["message"]
            .as_str()
            .unwrap()
            .contains("reserved")
    );
}
