use thiserror::Error;

#[derive(Debug, Error)]
pub enum SynthError {
    #[error("top module '{name}' not found")]
    TopModuleNotFound { name: String },

    #[error("unsupported construct in Phase 1: {what}")]
    Unsupported { what: String },

    #[error("unable to determine width for '{what}'")]
    UnknownWidth { what: String },

    #[error("dynamic index / select is not supported in Phase 1 ({what})")]
    DynamicSelect { what: String },

    #[error("multiple drivers for variable '{name}'")]
    MultipleDrivers { name: String },

    #[error("internal error: {0}")]
    Internal(String),
}

impl SynthError {
    pub fn unsupported(what: impl Into<String>) -> Self {
        SynthError::Unsupported { what: what.into() }
    }

    pub fn dynamic_select(what: impl Into<String>) -> Self {
        SynthError::DynamicSelect { what: what.into() }
    }
}
