use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doc {
    #[serde(default = "default_path")]
    pub path: PathBuf,
}

impl Default for Doc {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

fn default_path() -> PathBuf {
    "doc".into()
}
