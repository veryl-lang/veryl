use crate::OptUpdate;
use miette::Result;
use std::time::Instant;
use veryl_metadata::Metadata;

pub struct CmdUpdate {
    opt: OptUpdate,
}

impl CmdUpdate {
    pub fn new(opt: OptUpdate) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let now = Instant::now();

        let _paths = metadata.paths::<&str>(&[], true)?;

        let elapsed_time = now.elapsed();
        self.print(&format!(
            "[Info] Elapsed time: {} milliseconds.",
            elapsed_time.as_millis()
        ));

        Ok(true)
    }

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
    }
}
