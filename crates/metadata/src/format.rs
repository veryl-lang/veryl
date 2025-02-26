use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Format {
    #[serde(default = "default_indent_width")]
    pub indent_width: usize,

    #[serde(default = "default_max_width")]
    pub max_width: usize,
}

impl Default for Format {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

fn default_indent_width() -> usize {
    4
}

fn default_max_width() -> usize {
    120
}
