//! Hover fixtures, grouped by what the cursor is on:
//!
//! - `variable/`: the pattern of a `let` statement.
//! - `reference/`: an identifier referring to a local variable.
//! - `parameter/`: a parameter of a def or a closure, or the variable of a
//!   `handle` clause, where it is declared or referred to.
//! - `path/`: a segment of a path, which shows the signature of the symbol it
//!   names.
//! - `none/`: something that is neither a local variable nor a symbol, which
//!   shows nothing.

#![allow(missing_docs)]

use insta::assert_snapshot;
use ray_lsp_fixture::Fixture;
use serde_json::Value;

// The server is asked for the hover at the cursor marker, and the snapshot is
// its result.
#[test_generator::test_resources("lsp/e2e/test/hover/**/main.ray")]
fn main(resource: &str) {
    let fixture = Fixture::new(resource);
    let response = fixture.request_at_cursor("textDocument/hover");

    fixture.snapshot_settings().bind(|| {
        assert_snapshot!("snapshot", hover_result(&response));
    });
}

/// Renders the result of a hover response.
fn hover_result(response: &Value) -> String {
    serde_json::to_string_pretty(response.get("result").unwrap_or(&Value::Null)).unwrap()
}
