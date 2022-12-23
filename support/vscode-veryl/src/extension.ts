// The module 'vscode' contains the VS Code extensibility API
// Import the module and reference it with the alias vscode in your code below
import * as vscode from 'vscode';
import { commands, workspace, ExtensionContext, window, Uri } from 'vscode';

import {
	LanguageClient,
	LanguageClientOptions,
	ServerOptions,
	TransportKind
} from 'vscode-languageclient/node';

let client: LanguageClient;

function startServer() {
	let veryl_ls_binary_path: string | undefined = workspace.getConfiguration("vscode-veryl").get("verylLsBinary.path");
	if (typeof veryl_ls_binary_path === "undefined")	veryl_ls_binary_path = "veryl-ls";
	// If the extension is launched in debug mode then the debug server options are used
	// Otherwise the run options are used
	let serverOptions: ServerOptions = {
		run: {command: veryl_ls_binary_path},
		debug: {command: veryl_ls_binary_path},
	};

	// Options to control the language client
	let clientOptions: LanguageClientOptions = {
		// Register the server for plain text documents
		documentSelector: [{ scheme: 'file', language: 'veryl' }],
	};

	// Create the language client and start the client.
	client = new LanguageClient(
		'veryl-ls',
		'Veryl language server',
		serverOptions,
		clientOptions
	);

	// Start the client. This will also launch the server
	client.start();
}

function stopServer(): Thenable<void> {
	if (!client) {
		return Promise.resolve();
	}
	return client.stop();
}

// This method is called when your extension is activated
// Your extension is activated the very first time the command is executed
export function activate(context: vscode.ExtensionContext) {

	// Use the console to output diagnostic information (console.log) and errors (console.error)
	// This line of code will only be executed once when your extension is activated
	console.log('Congratulations, your extension "vscode-veryl" is now active!');

	context.subscriptions.push(
		commands.registerCommand("vscode-veryl.restartVerylLs", () => {
			stopServer().then(startServer, startServer);
		})
	)

	startServer();
}

// This method is called when your extension is deactivated
export function deactivate(): Thenable<void> {
	return stopServer();
}
