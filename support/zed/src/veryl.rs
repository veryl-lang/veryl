use zed::LanguageServerId;
use zed_extension_api::{self as zed, Worktree, serde_json, settings::LspSettings};

struct VerylExtension {}

impl zed::Extension for VerylExtension {
    fn new() -> Self {
        Self {}
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed_extension_api::Worktree,
    ) -> zed_extension_api::Result<zed_extension_api::Command> {
        match worktree.which("veryl-ls") {
            Some(path) => Ok(zed::Command {
                command: path,
                args: vec![],
                env: vec![],
            }),
            None => Err(
                "veryl-ls is not installed. Please install it using 'cargo install veryl-ls'."
                    .into(),
            ),
        }
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed_extension_api::Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.initialization_options.clone())
            .unwrap_or_default();
        Ok(Some(settings))
    }
}

zed::register_extension!(VerylExtension);
