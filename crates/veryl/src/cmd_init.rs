use crate::utils;
use crate::OptInit;
use miette::{bail, IntoDiagnostic, Result};
use std::fs::File;
use std::io::Write;

pub struct CmdInit {
    opt: OptInit,
}

impl CmdInit {
    pub fn new(opt: OptInit) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        if !self.opt.path.exists() {
            bail!(
                "path \"{}\" does not exist",
                self.opt.path.to_string_lossy()
            );
        }

        if let Some(name) = self.opt.path.canonicalize().into_diagnostic()?.file_name() {
            let toml = utils::create_default_toml(&name.to_string_lossy());
            let toml_path = self.opt.path.join("Veryl.toml");

            if toml_path.exists() {
                bail!("\"{}\" exists", toml_path.to_string_lossy());
            }

            let mut file = File::create(toml_path).into_diagnostic()?;
            write!(file, "{}", toml).into_diagnostic()?;
            file.flush().into_diagnostic()?;

            println!("Created \"{}\" package", name.to_string_lossy());
        } else {
            bail!("path \"{}\" is not valid", self.opt.path.to_string_lossy());
        }

        Ok(true)
    }
}
