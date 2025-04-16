use crate::OptInit;
use log::info;
use miette::{IntoDiagnostic, Result, bail};
use std::fs::{self, File};
use std::io::Write;
use veryl_metadata::{Git, Metadata};

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
            let name = name.to_string_lossy();

            let toml = Metadata::create_default_toml(&name).into_diagnostic()?;
            let toml_path = self.opt.path.join("Veryl.toml");

            if toml_path.exists() {
                bail!("\"{}\" exists", toml_path.to_string_lossy());
            }

            let mut file = File::create(toml_path).into_diagnostic()?;
            write!(file, "{toml}").into_diagnostic()?;
            file.flush().into_diagnostic()?;

            let src_path = self.opt.path.join("src");
            fs::create_dir_all(&src_path).into_diagnostic()?;

            let gitignore = Metadata::create_default_gitignore();
            let gitignore_path = self.opt.path.join(".gitignore");

            if Git::exists() {
                if !gitignore_path.exists() {
                    let mut file = File::create(&gitignore_path).into_diagnostic()?;
                    write!(file, "{gitignore}").into_diagnostic()?;
                    file.flush().into_diagnostic()?;
                }

                Git::init(&self.opt.path)?;
            }

            info!("Created \"{}\" project", name);
        } else {
            bail!("path \"{}\" is not valid", self.opt.path.to_string_lossy());
        }

        Ok(true)
    }
}
