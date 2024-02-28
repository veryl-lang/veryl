use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doc {
    #[serde(default = "default_path")]
    pub path: PathBuf,
}

fn default_path() -> PathBuf {
    "doc".into()
}
