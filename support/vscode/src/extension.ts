// The module 'vscode' contains the VS Code extensibility API
// Import the module and reference it with the alias vscode in your code below
import { cpSync } from 'fs';
import { type } from 'os';
import { format } from 'path';
import * as vscode from 'vscode';
import { commands, workspace, ExtensionContext, window, Uri } from 'vscode';
import * as path from 'path';

import {
	LanguageClient,
	LanguageClientOptions,
	ServerOptions,
	TransportKind
} from 'vscode-languageclient/node';
import { start } from 'repl';

let client: LanguageClient;

function startServer(context: vscode.ExtensionContext) {
	let verylLsIntegrated = context.asAbsolutePath(path.join('bin', 'veryl-ls'));

	let verylLsBinaryPath: string | undefined = workspace.getConfiguration("vscode-veryl").get("verylLsBinary.path");
	if (typeof verylLsBinaryPath === "undefined") {
		verylLsBinaryPath = verylLsIntegrated;
	} else if (verylLsBinaryPath === null) {
		verylLsBinaryPath = verylLsIntegrated;
	}

	// If the extension is launched in debug mode then the debug server options are used
	// Otherwise the run options are used
	let serverOptions: ServerOptions = {
		run: {command: verylLsBinaryPath},
		debug: {command: verylLsBinaryPath},
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
			stopServer().then(function () {startServer(context)}, startServer);
		})
	)

	startServer(context);
}

// This method is called when your extension is deactivated
export function deactivate(): Thenable<void> {
	return stopServer();
}
