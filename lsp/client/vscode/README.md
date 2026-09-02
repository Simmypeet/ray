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

The client starts `rayc lsp` by default. Set `ray.server.path` and
`ray.server.arguments` if the language server is installed elsewhere or uses a
different command line.
