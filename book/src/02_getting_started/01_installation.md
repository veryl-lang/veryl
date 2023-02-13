# Installation

You can install Veryl by downloading binary.
If you have Rust development environment, you can use `cargo` instead of it.

## Requirement

Veryl uses `git` command internally. Please confirm `git` can be launched.

## Choose a way of installation

### Download binary

Download from [release page](https://github.com/dalance/veryl/releases/latest), and extract to the directory in PATH.

### Cargo

You can install with [cargo](https://crates.io/crates/veryl).

```
cargo install veryl veryl-ls
```

## Editor integration

[Visual Studio Code](https://azure.microsoft.com/ja-jp/products/visual-studio-code) and [Vim](https://github.com/vim/vim) / [Neovim](https://neovim.io) are supported officially.

### Visual Studio Code

For Visual Studio Code, Veryl extension is provided.
The extension provides file type detection, syntex highlight and language server integration.
You can install it by searching "Veryl" in extension panel or the following URL.

[Veryl extension for Visual Studio Code](https://marketplace.visualstudio.com/items?itemName=dalance.vscode-veryl)

### Vim / Neovim

For Vim / Neovim, Veryl plugin is provided.
The plugin provides file type detection, syntex highlight.
There are some instructions for plugin installation and language server integration in the following URL.

[Vim / Neovim plugin](https://github.com/dalance/veryl.vim)

### Other Editors

Veryl provides language server. So other editors supporting language server (ex. Emacs) can use it.
