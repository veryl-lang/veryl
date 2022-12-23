use crate::OptMetadata;
use veryl_metadata::Metadata;
use veryl_parser::miette::Result;

pub struct CmdMetadata {
    opt: OptMetadata,
}

impl CmdMetadata {
    pub fn new(opt: OptMetadata) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        println!("{:?}", metadata);

        Ok(true)
    }

    fn print(&self, msg: &str) {
        if self.opt.verbose {
            println!("{}", msg);
        }
    }
}
