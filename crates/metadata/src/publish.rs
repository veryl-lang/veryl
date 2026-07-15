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
    /// Whether `veryl publish` registers the project with the Veryl registry.
    /// `Some(true)` registers automatically, `Some(false)` never registers, and
    /// `None` (unset) asks once interactively.
    #[serde(default)]
    pub register: Option<bool>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_tri_state() {
        let on: Publish = toml::from_str("register = true").unwrap();
        assert_eq!(on.register, Some(true));

        let off: Publish = toml::from_str("register = false").unwrap();
        assert_eq!(off.register, Some(false));

        let unset: Publish = toml::from_str("").unwrap();
        assert_eq!(unset.register, None);
    }
}
