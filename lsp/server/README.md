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

The initial server supports full document synchronization and publishes all
diagnostics from the same query as `ray check <file>` on open and every text
change. Checks use unsaved editor contents and reuse one in-memory compiler
engine per open document across edits.
No source files are written. Diagnostic positions use UTF-16, and closing a
document clears its diagnostics and releases its compiler state.

Each file URI is checked as an independent compilation root. Workspace/project
discovery, cross-file editing, non-file URIs, and other language features are
not supported yet. Checks are serialized; there is no debounce or cancellation
in this initial version.

The server follows tower-lsp's [document synchronization API](https://docs.rs/tower-lsp/0.20.0/tower_lsp/trait.LanguageServer.html#method.did_change).

Run `cargo test -p ray_lsp` for the stdio integration test.
