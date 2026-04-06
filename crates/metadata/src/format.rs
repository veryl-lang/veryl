use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewlineStyle {
    #[default]
    Auto,
    Native,
    Unix,
    Windows,
}

impl NewlineStyle {
    pub fn newline_str(&self, input: &str) -> &'static str {
        match self {
            NewlineStyle::Auto => auto_detect_newline_style(input),
            NewlineStyle::Native => native_newline_str(),
            NewlineStyle::Unix => "\n",
            NewlineStyle::Windows => "\r\n",
        }
    }
}

fn auto_detect_newline_style(input: &str) -> &'static str {
    let first_lf = input.find('\n');
    match first_lf {
        Some(pos) if pos > 0 && input.as_bytes()[pos - 1] == b'\r' => "\r\n",
        Some(_) => "\n",
        None => native_newline_str(),
    }
}

fn native_newline_str() -> &'static str {
    if cfg!(target_os = "windows") {
        "\r\n"
    } else {
        "\n"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Format {
    #[serde(default = "default_indent_width")]
    pub indent_width: usize,

    #[serde(default = "default_max_width")]
    pub max_width: usize,

    #[serde(default = "default_vertical_align")]
    pub vertical_align: bool,

    #[serde(default)]
    pub newline_style: NewlineStyle,
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

fn default_vertical_align() -> bool {
    true
}
