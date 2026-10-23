//! Completion fixtures, grouped by what is completed:
//!
//! - `keyword/`: the keywords the syntax allows at the cursor.
//! - `name/`: the local variables at a name in a body.
//! - `field/`: the fields and tuple indices after a `.` in a body.
//! - `path/segment/`: the segment after a `.` in a path.
//! - `path/typing/`: a path being typed in a body, before its `(`.
//! - `path/first/`: the first segment of a path.
//! - `path/kind/`: what fits where the path is written, such as a type.
//! - `path/trait_member/`: the members reached through `this` or an instance
//!   parameter.

#![allow(missing_docs)]

use insta::assert_snapshot;
use ray_lsp_fixture::Fixture;
use serde_json::Value;

// The server is asked for the completions at the cursor marker, and the
// snapshot is its response with the items in a canonical order.
#[test_generator::test_resources("lsp/e2e/test/completion/**/main.ray")]
fn main(resource: &str) {
    let fixture = Fixture::new(resource);
    let response = fixture.request_at_cursor("textDocument/completion");

    fixture.snapshot_settings().bind(|| {
        assert_snapshot!("snapshot", canonical_completions(&response));
    });
}

/// Renders the result of a completion response, with its items sorted by
/// label, then by the rest of the item.
fn canonical_completions(response: &Value) -> String {
    let mut result = response.get("result").cloned().unwrap_or(Value::Null);

    // the result is a list of items, or a list that may be incomplete
    let items = match &mut result {
        Value::Array(items) => Some(items),
        Value::Object(list) => list.get_mut("items").and_then(Value::as_array_mut),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    };
    if let Some(items) = items {
        items.sort_by_cached_key(|item| (item["label"].to_string(), item.to_string()));
    }

    serde_json::to_string_pretty(&result).unwrap()
}
