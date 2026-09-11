import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

function createClient(): LanguageClient {
  const configuration = vscode.workspace.getConfiguration("ray.server");
  const command = configuration.get<string>("path", "rayc");
  const args = configuration.get<string[]>("arguments", ["lsp"]);
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
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.ray"),
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

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("ray.restartLanguageServer", async () => {
      await stopClient();
      await startClient();
    }),
  );

  await startClient();
}

export async function deactivate(): Promise<void> {
  await stopClient();
}
