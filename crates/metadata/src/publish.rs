use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    #[serde(default)]
    pub bump_commit: bool,
    #[serde(default)]
    pub publish_commit: bool,
    #[serde(default = "default_bump_commit_message")]
    pub bump_commit_message: String,
    #[serde(default = "default_publish_commit_message")]
    pub publish_commit_message: String,
}

impl Default for Publish {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

fn default_bump_commit_message() -> String {
    "chore: Bump version".to_string()
}

fn default_publish_commit_message() -> String {
    "chore: Publish".to_string()
}
