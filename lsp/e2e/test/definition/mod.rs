//! Go to definition fixtures, grouped by what the cursor is on:
//!
//! - `path/`: a segment of a path, which goes to the name of the symbol it
//!   names.
//! - `local/`: a local variable or parameter, which goes to the pattern
//!   declaring it.
//! - `none/`: something that is neither a local variable nor a symbol, which
//!   goes nowhere.

#![allow(missing_docs)]

use insta::assert_snapshot;
use ray_lsp_fixture::Fixture;
use serde_json::Value;

// The server is asked for the definition at the cursor marker, and the
// snapshot is its result.
#[test_generator::test_resources("lsp/e2e/test/definition/**/main.ray")]
fn main(resource: &str) {
    let fixture = Fixture::new(resource);
    let response = fixture.request_at_cursor("textDocument/definition");

    fixture.snapshot_settings().bind(|| {
        assert_snapshot!("snapshot", definition_result(&response));
    });
}

/// Renders the result of a definition response.
fn definition_result(response: &Value) -> String {
    serde_json::to_string_pretty(response.get("result").unwrap_or(&Value::Null)).unwrap()
}
