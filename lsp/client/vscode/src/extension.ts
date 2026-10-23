import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

function createClient(): LanguageClient {
  const configuration = vscode.workspace.getConfiguration("ray.server");
  const command = configuration.get<string>("path", "ray_lsp");
  const args = configuration.get<string[]>("arguments", []);
  const serverOptions: ServerOptions = {
    command,
    args,
    options: {
      cwd: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
    },
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: "ray", scheme: "file" }],
    synchronize: {
      // source files changed on disk and project manifests
      fileEvents: vscode.workspace.createFileSystemWatcher("**/{*.ray,ray.toml}"),
    },
  };

  return new LanguageClient(
    "ray",
    "Ray Language Server",
    serverOptions,
    clientOptions,
  );
}

async function startClient(): Promise<void> {
  if (client !== undefined) {
    return;
  }

  client = createClient();
  await client.start();
}

async function stopClient(): Promise<void> {
  const runningClient = client;
  client = undefined;

  if (runningClient !== undefined) {
    await runningClient.stop();
  }
}

// The bundled core library has no file; diagnostics refer to it by this
// scheme, and its text comes from the server.
const CORE_SCHEME = "ray-core";
const CORE_SOURCE_REQUEST = "ray/coreSource";

const coreSourceProvider: vscode.TextDocumentContentProvider = {
  async provideTextDocumentContent(): Promise<string> {
    if (client === undefined) {
      throw new Error("the Ray language server is not running");
    }

    return client.sendRequest<string>(CORE_SOURCE_REQUEST);
  },
};

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("ray.restartLanguageServer", async () => {
      await stopClient();
      await startClient();
    }),
    vscode.workspace.registerTextDocumentContentProvider(CORE_SCHEME, coreSourceProvider),
  );

  await startClient();
}

export async function deactivate(): Promise<void> {
  await stopClient();
}
