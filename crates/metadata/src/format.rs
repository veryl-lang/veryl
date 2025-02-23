use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Format {
    #[serde(default = "default_indent_width")]
    pub indent_width: usize,
}

const DEFAULT_INDENT_WIDTH: usize = 4;

impl Default for Format {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

fn default_indent_width() -> usize {
    DEFAULT_INDENT_WIDTH
}
