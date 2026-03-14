use crate::HashMap;
use crate::ir::Config;
use crate::ir::ProtoStatement;
use crate::ir::VarId;
use crate::ir::VariableMeta;
use memmap2::Mmap;

pub struct ScopeContext {
    pub variable_meta: HashMap<VarId, VariableMeta>,
    pub analyzer_context: veryl_analyzer::conv::Context,
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    pub scope_contexts: Vec<ScopeContext>,
    pub binary: Vec<Mmap>,
    pub ff_total_count: usize,
    pub comb_total_count: usize,
    pub pending_statements: Vec<ProtoStatement>,
}

impl Context {
    pub fn scope(&mut self) -> &mut ScopeContext {
        self.scope_contexts.last_mut().unwrap()
    }
}

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, src: T) -> Option<Self>;
}
