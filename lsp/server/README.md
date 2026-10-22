# Ray language server

Run `cargo run -p ray_lsp` or configure an LSP client to launch the built
`target/debug/ray_lsp` executable. The server communicates over stdio.

The VS Code extension launches `ray_lsp` from `PATH` with no arguments by default.
To use a local build, configure:

```json
{
  "ray.server.path": "/absolute/path/to/ray/target/debug/ray_lsp",
  "ray.server.arguments": []
}
```

The server works only in project mode: an open document is checked as part of
the project described by the nearest `ray.toml` in its directory or a parent
directory. For a file outside any project, the server shows a message
suggesting `rayc new`. Each project has one compiler engine, which publishes
the diagnostics of every file in the project, from the same query as
`rayc check`.

Checks use the unsaved contents of open documents and the saved contents of
every other file. No source files are written. Documents are synchronized
incrementally: each edit updates the document's rope in place. Positions in
edits and diagnostics use UTF-16.
Checks are debounced, and an edit cancels the check in progress.

Completion offers what the segment of a path at the cursor can name,
wherever a path is written: in types, effect rows, where and given clauses,
instance headers, and in the calls and struct initializations of a body. It
keeps what fits the position, such as types in a type. Inside a `def` body it
also offers the local variables visible at the cursor, with their types, and
after a `.` the fields of a struct that the `def` can access, or the indices
of a tuple. A `.` also triggers it. Completion reads the unsaved contents too:
a completion request stops a pending check, re-reads the documents, and
schedules the check again. Run `cargo test -p rayc_e2e --test completion_e2e`
for the completion fixtures.

The engine caches its compilation in `target/lsp` in the project directory,
next to the compiler's `target/inc`, so a restarted server recomputes only what
changed. The two caches are separate because a cache can be open in only one
process at a time. If the cache cannot be opened, for example because another
server holds it, the server caches in memory and shows a warning. Unsaved
contents never reach the cache.

Diagnostics may refer to the bundled core library, which has no file, by the
URI `ray-core:/core.ray`. A client fetches its text with the `ray/coreSource`
request, which takes no parameters; the VS Code extension serves it as a
read-only document.

The client should send `workspace/didChangeWatchedFiles` for `*.ray` and
`ray.toml` files so that changes on disk and manifest edits are picked up.

The server follows tower-lsp's [document synchronization API](https://docs.rs/tower-lsp/0.20.0/tower_lsp/trait.LanguageServer.html#method.did_change).

Run `cargo test -p ray_lsp` for the stdio integration test.
