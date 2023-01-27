use crate::{Format, OptMetadata};
use miette::{IntoDiagnostic, Result};
use veryl_metadata::Metadata;

pub struct CmdMetadata {
    opt: OptMetadata,
}

impl CmdMetadata {
    pub fn new(opt: OptMetadata) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let text = match self.opt.format {
            Format::Json => serde_json::to_string(metadata).into_diagnostic()?,
            Format::Pretty => format!("{metadata:#?}"),
        };

        println!("{text}");

        Ok(true)
    }
}
