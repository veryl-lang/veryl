use crate::utils;
use crate::OptNew;
use miette::{bail, IntoDiagnostic, Result};
use std::fs::{self, File};
use std::io::Write;

pub struct CmdNew {
    opt: OptNew,
}

impl CmdNew {
    pub fn new(opt: OptNew) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        if self.opt.path.exists() {
            bail!("path \"{}\" exists", self.opt.path.to_string_lossy());
        }

        if let Some(name) = self.opt.path.file_name() {
            let toml = utils::create_default_toml(&name.to_string_lossy());

            fs::create_dir_all(&self.opt.path).into_diagnostic()?;
            let mut file = File::create(self.opt.path.join("Veryl.toml")).into_diagnostic()?;
            write!(file, "{}", toml).into_diagnostic()?;
            file.flush().into_diagnostic()?;

            println!("Created \"{}\" package", name.to_string_lossy());
        } else {
            bail!("path \"{}\" is not valid", self.opt.path.to_string_lossy());
        }

        Ok(true)
    }
}
