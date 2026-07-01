use crate::OptDump;
use crate::pipeline::{self, AnalyzeOptions};
use miette::Result;
use veryl_analyzer::ir::Ir;
use veryl_metadata::Metadata;

pub struct CmdDump {
    opt: OptDump,
}

impl CmdDump {
    pub fn new(opt: OptDump) -> Self {
        Self { opt }
    }

    pub fn exec(&self, metadata: &mut Metadata) -> Result<bool> {
        let paths = metadata.paths(&self.opt.files, true, true)?;

        // Debug tool: analyze a broken tree best-effort (`fail_fast: false`).
        let options = AnalyzeOptions {
            defines: &[],
            emit_mode: false,
            incremental: false,
            fail_fast: false,
        };
        let mut ir = Ir::default();
        let _ = pipeline::analyze(metadata, &paths, options, Some(&mut ir), None)?;

        if self.opt.symbol_table {
            println!("{}", veryl_analyzer::symbol_table::dump());
        }

        if self.opt.namespace_table {
            println!("{}", veryl_analyzer::scope::dump_tokens());
        }

        if self.opt.type_dag {
            println!("{}", veryl_analyzer::type_dag::dump());
        }

        if self.opt.file_dag {
            println!("{}", veryl_analyzer::type_dag::dump_file());
        }

        if self.opt.attribute_table {
            println!("{}", veryl_analyzer::attribute_table::dump());
        }

        if self.opt.unsafe_table {
            println!("{}", veryl_analyzer::unsafe_table::dump());
        }

        if self.opt.ir {
            println!("{}", ir);
        }

        Ok(true)
    }
}
