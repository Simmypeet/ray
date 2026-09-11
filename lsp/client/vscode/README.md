# Ray Language Support for VS Code

This extension provides basic language support for `.ray` files:

- TextMate-based syntax highlighting
- indentation, bracket, comment, and folding configuration
- a Language Server Protocol client

## Development

Install dependencies and compile the extension:

```sh
npm install
npm run compile
```

Open this directory in VS Code and press `F5` to launch an Extension Development Host.

## Language server

The client starts `ray_lsp` from your `PATH` with no arguments and communicates
over stdio. Build the server from the repository root with `cargo build -p ray_lsp`.
To use that build, set `ray.server.path` to the absolute path of
`target/debug/ray_lsp`.

If you previously configured `rayc lsp`, update `ray.server.path` and remove the
`ray.server.arguments` override (or set it to `[]`). Then run
**Ray: Restart Language Server**.

## Packaging and installation

With the `@vscode/vsce` CLI installed, run from this directory:

```sh
npm ci
npm run package
code --install-extension ray-language-support-0.1.0.vsix --force
```

Then run **Developer: Reload Window** in VS Code.

The compiled extension uses `vscode-languageclient` at runtime, so its production
dependencies must be included in the VSIX. Do not exclude `node_modules` in
`.vscodeignore` unless the extension is changed to bundle those dependencies.
An extension-host error such as `Cannot find module 'vscode-languageclient/node'`
means the installed package is incomplete; installing `ray_lsp` alone cannot fix it.
