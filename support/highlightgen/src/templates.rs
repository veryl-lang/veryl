mod ace;
mod highlightjs;
mod vim;
mod vscode;
pub use ace::Ace;
pub use highlightjs::Highlightjs;
pub use vim::Vim;
pub use vscode::Vscode;

use crate::keywords::Keywords;
use std::path::PathBuf;

pub trait Template {
    fn apply(&self, keywords: &Keywords) -> String;
    fn path(&self) -> PathBuf;
}
