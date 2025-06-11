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
		commands.registerCommand("vscode-veryl.restartServer", () => {
			stopServer().then(function () {startServer(context);}, startServer);
		})
	);
  // Added by Kalyan for Waveform Render start
	// Start and live preview mode
	context.subscriptions.push(
		vscode.commands.registerCommand("waveformRender.start", () => {
		WaveformRenderPanel.disableLivePreview();
		vscode.window.showInformationMessage(
			"Waveform refreshed manually, Live Preview OFF"
		);
		WaveformRenderPanel.createOrShow(context.extensionPath);
		})
	);
	context.subscriptions.push(
		vscode.commands.registerCommand("waveformRender.toggleLivePreview", () => {
		WaveformRenderPanel.toggleLivePreview(context.extensionPath);
		})
	);

	// Add listener for changing active text editor
	context.subscriptions.push(
		vscode.window.onDidChangeActiveTextEditor((editor) => {
		if (
			WaveformRenderPanel.livePreview &&
			editor &&
			(editor.document.fileName.toLowerCase().endsWith(".veryl"))
		) {
			WaveformRenderPanel.createOrShow(context.extensionPath);
		}
		})
	);

	// Export the waveform
	context.subscriptions.push(
		vscode.commands.registerCommand("waveformRender.saveAsPng", () => {
		WaveformRenderPanel.saveAsPng();
		})
	);
	context.subscriptions.push(
		vscode.commands.registerCommand("waveformRender.saveAsSvg", () => {
		WaveformRenderPanel.saveAsSvg();
		})
	);
	// Added by Kalyan for Waveform Render start

	startServer(context);
}

// This method is called when your extension is deactivated
export function deactivate(): Thenable<void> {
	return stopServer();
}


// Added by kalyan for Waveform Render start
function getFilename() {
  const editor = vscode.window.activeTextEditor;
  if (!editor || !editor.document) {
    return "untitled";
  }
  const fileName = editor.document.fileName;
  if (!fileName) {
    return "untitled";
  }
  return fileName
    .split(/[\\/]/)
    .pop()
    ?.replace(/\.veryl?$/i, "") ?? "untitled";
}

function getTitle() {
  return "Waveform: " + getFilename();
}

/**
 * Manages webview panel
 */
class WaveformRenderPanel {
  /**
   * Track the currently panel. Only allow a single panel to exist at a time.
   */
  public static currentPanel: WaveformRenderPanel | undefined;

  public static livePreview: boolean = false;
  public static livePreviewDocumentPath: string | null;
  public static listenerTextChange: vscode.Disposable | undefined;

  public static readonly viewType = "waveformRender";

  private readonly _panel: vscode.WebviewPanel;
  private readonly _extensionPath: string;
  private _disposables: vscode.Disposable[] = [];

  public static toggleLivePreview(extensionPath: string) {
    const closePanelOnDisable = vscode.workspace
      .getConfiguration("waveformRender")
      .get<boolean>("closePanelOnDisable", true);

    if (WaveformRenderPanel.livePreview) {
      WaveformRenderPanel.disableLivePreview();

      // Close the panel if the setting is enabled
      if (closePanelOnDisable && WaveformRenderPanel.currentPanel) {
        WaveformRenderPanel.currentPanel.dispose();
      }
    } else {
      WaveformRenderPanel.livePreviewDocumentPath =
        vscode.window.activeTextEditor && vscode.window.activeTextEditor.document
          ? vscode.window.activeTextEditor.document.uri.path
          : null;
      WaveformRenderPanel.listenerTextChange =
        vscode.workspace.onDidChangeTextDocument(function (event) {
          WaveformRenderPanel.createOrShow(extensionPath);
        });
      WaveformRenderPanel.livePreview = true;
      WaveformRenderPanel.createOrShow(extensionPath);
    }
    vscode.window.showInformationMessage(
      "Waveform Live Preview: " +
        (WaveformRenderPanel.livePreview ? "ON" : "OFF")
    );
  }

  public static disableLivePreview() {
    WaveformRenderPanel.livePreviewDocumentPath = null;
    if (WaveformRenderPanel.listenerTextChange) {
      WaveformRenderPanel.listenerTextChange.dispose();
    }
    WaveformRenderPanel.livePreview = false;
  }

  public static createOrShow(extensionPath: string) {
    const activeEditor = vscode.window.activeTextEditor;

    // Ensure we have an active editor and it's a JSON file
    if (
      !activeEditor ||
      !(activeEditor.document.fileName.toLowerCase().endsWith(".veryl"))
    ) {
      return;
    }

    // If we already have a panel
    if (WaveformRenderPanel.currentPanel) {
      // Update the panel title
      WaveformRenderPanel.currentPanel._panel.title = getTitle();

      // Update the panel content
      WaveformRenderPanel.currentPanel._updateWithFileContent();
      return;
    }

    // Otherwise, create a new panel.
    const panel = vscode.window.createWebviewPanel(
      WaveformRenderPanel.viewType,
      getTitle(),
      { preserveFocus: true, viewColumn: vscode.ViewColumn.Beside },
      {
        // Enable javascript in the webview
        enableScripts: true,

        // And restrict the webview to only loading content from our extension's `localScripts` directory.
        localResourceRoots: [
          vscode.Uri.file(path.join(extensionPath, "localScripts")),
        ],
      }
    );

    WaveformRenderPanel.currentPanel = new WaveformRenderPanel(
      panel,
      extensionPath
    );
  }

  private constructor(panel: vscode.WebviewPanel, extensionPath: string) {
    this._panel = panel;
    this._extensionPath = extensionPath;

    this._updateWithFileContent();

    // Listen for when the panel is disposed
    // This happens when the user closes the panel or when the panel is closed programatically
    this._panel.onDidDispose(() => this.dispose(), null, this._disposables);
  }

  public dispose() {
    WaveformRenderPanel.currentPanel = undefined;

    // Disable live preview when the panel is closed
    WaveformRenderPanel.disableLivePreview();

    // Clean up our resources
    this._panel.dispose();

    while (this._disposables.length) {
      const x = this._disposables.pop();
      if (x) {
        x.dispose();
      }
    }
  }

  public static saveAsSvg() {
    if (WaveformRenderPanel.currentPanel) {
      WaveformRenderPanel.currentPanel._panel.webview.postMessage({
        command: "saveSvg",
      });
    }
  }

  public static saveAsPng() {
    if (WaveformRenderPanel.currentPanel) {
      WaveformRenderPanel.currentPanel._panel.webview.postMessage({
        command: "savePng",
      });
    }
  }

  private _updateWithFileContent() {
    // Get the current text editor
    let editor = vscode.window.activeTextEditor;
    if (!editor) {
      vscode.window.showErrorMessage("No active editor found.");
      return;
    }
    let doc = editor.document;
    let docContent = doc.getText();
  const match = docContent.match(/```wavedrom([\s\S]*?)```/);
  const wavedromContent = match ? match[1].trim() : "";
  const cleanedContent = wavedromContent.replace(/\//g, '');
  this._update(cleanedContent, getFilename());
  }

  private _update(
    fileContents: string = `{ signal: [
    { name: "clk",         wave: "p.....|..." },
    { name: "Data",        wave: "x.345x|=.x", data: ["head", "body", "tail", "data"] },
    { name: "Request",     wave: "0.1..0|1.0" },
    {},
    { name: "Acknowledge", wave: "1.....|01." }
  ]}`,
    title?: string
  ) {
    this._panel.webview.html = this._getHtmlForWebview(fileContents, title);
  }

  private _getHtmlForWebview(
    waveformJson: string,
    title: string = "waveform render"
  ) {
    const scriptPathOnDisk = vscode.Uri.file(
      path.join(this._extensionPath, "localScripts", "wavedrom.min.js")
    );
    const defaultSkinPathOnDisk = vscode.Uri.file(
      path.join(this._extensionPath, "localScripts/skins", "default.js")
    );
    const narrowSkinPathOnDisk = vscode.Uri.file(
      path.join(this._extensionPath, "localScripts/skins", "narrow.js")
    );
    const lowkeySkinPathOnDisk = vscode.Uri.file(
      path.join(this._extensionPath, "localScripts/skins", "lowkey.js")
    );

    // And the uri we use to load this script in the webview
    const scriptUri = this._panel.webview.asWebviewUri(scriptPathOnDisk);
    const defaultUri = this._panel.webview.asWebviewUri(defaultSkinPathOnDisk);
    const narrowUri = this._panel.webview.asWebviewUri(narrowSkinPathOnDisk);
    const lowkeyUri = this._panel.webview.asWebviewUri(lowkeySkinPathOnDisk);

    return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">

                  <script src="${scriptUri}"></script>

                  <script src="${defaultUri}"></script>
                  <script src="${narrowUri}"></script>
                  <script src="${lowkeyUri}"></script>

                  <title>${title}</title>
            </head>

            <script>
            window.addEventListener('message', async event => {
              const command = event.data.command;

              const svgEl = document.querySelector('svg');
              if (!svgEl) return;

              if (command === 'saveSvg') {
                const blob = new Blob([svgEl.outerHTML], { type: 'image/svg+xml' });
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = document.title + '.svg';
                a.click();
                URL.revokeObjectURL(url);
              }

              if (command === 'savePng') {
                const svg = new XMLSerializer().serializeToString(svgEl);
                const svg64 = btoa(unescape(encodeURIComponent(svg)));
                const img = new Image();
                img.src = 'data:image/svg+xml;base64,' + svg64;

                img.onload = async function () {
                  const scaleFactor = 2; // 2x resolution
                  const canvas = document.createElement('canvas');
                  const width = img.width * scaleFactor;
                  const height = img.height * scaleFactor;

                  canvas.width = width;
                  canvas.height = height;
                  const ctx = canvas.getContext('2d');
                  ctx.scale(scaleFactor, scaleFactor); // scale the context to increase resolution
                  ctx.drawImage(img, 0, 0, img.width, img.height, 0, 0, img.width, img.height);

                  const pngUrl = canvas.toDataURL('image/png');

                  const a = document.createElement('a');
                  a.href = pngUrl;
                  a.download = document.title + '.png';
                  a.click();
                };
              }

            });
            </script>

            <body onload="WaveDrom.ProcessAll()" style="background-color: white;">
              <div id="copyBtn" style="display: flex; align-items: center; justify-content: flex-end; cursor: pointer; margin-top: 10px; margin-bottom: 10px;">
                <span style="font-size: 14px; margin-right: 3px;">📋</span>
                <span style="font-weight: 600; font-size: 16px;">copy to clipboard</span>
              </div>

              <div>
                <script type="WaveDrom">
                  ${waveformJson}
                </script>
              </div>

              <script>
                document.getElementById('copyBtn').addEventListener('click', async () => {
                  const svgEl = document.querySelector('svg');
                  if (!svgEl) {
                    alert('SVG not found!');
                    return;
                  }

                  const svg = new XMLSerializer().serializeToString(svgEl);
                  const svg64 = btoa(unescape(encodeURIComponent(svg)));
                  const img = new Image();
                  img.src = 'data:image/svg+xml;base64,' + svg64;

                  img.onload = async function () {
                    const scaleFactor = 2; // 2x resolution
                    const canvas = document.createElement('canvas');
                    const width = img.width * scaleFactor;
                    const height = img.height * scaleFactor;

                    canvas.width = width;
                    canvas.height = height;
                    const ctx = canvas.getContext('2d');
                    ctx.scale(scaleFactor, scaleFactor); // scale the context to increase resolution
                    ctx.drawImage(img, 0, 0, img.width, img.height, 0, 0, img.width, img.height);

                    const pngUrl = canvas.toDataURL('image/png');
                    const blob = await (await fetch(pngUrl)).blob();

                    try {
                      await navigator.clipboard.write([
                        new ClipboardItem({ [blob.type]: blob })
                      ]);
                      alert('Image copied to clipboard!');
                    } catch (err) {
                      alert('Clipboard copy failed: ' + err.message);
                    }
                  };
                });
              </script>

            </body>
            </html>`;
  }
}
