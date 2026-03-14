mod context;
mod declaration;
mod event;
mod expression;
mod module;
mod statement;
mod variable;

pub use context::{Context, Conv};
pub use declaration::ProtoDeclaration;
pub use event::Event;
pub use expression::{Expression, ProtoExpression};
pub use module::{Module, ProtoModule};
pub use statement::{
    ProtoStatement, ProtoStatementBlock, ProtoStatements, Statement, parse_hex_content,
};
pub use variable::{
    CombValue, FfValue, ModuleVariableMeta, ModuleVariables, Variable, VariableElement,
    VariableMeta, create_variable_meta,
};
pub use veryl_analyzer::ir::{Op, Type, VarId, VarPath};
pub use veryl_analyzer::value::Value;

use crate::HashMap;
use memmap2::Mmap;
use veryl_analyzer::ir as air;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;

pub struct Ir {
    pub name: StrId,
    pub ports: HashMap<VarPath, VarId>,
    pub ff_values: Box<[FfValue]>,
    pub comb_values: Box<[CombValue]>,
    pub module_variables: ModuleVariables,
    pub event_statements: HashMap<Event, Vec<Statement>>,
    pub comb_statements: Vec<Statement>,
    /// Keeps JIT-compiled code alive as long as the Ir instance is alive.
    _binary: Vec<Mmap>,
}

impl Ir {
    pub fn from_module(module: Module, binary: Vec<Mmap>) -> Ir {
        Ir {
            name: module.name,
            ports: module.ports,
            ff_values: module.ff_values,
            comb_values: module.comb_values,
            module_variables: module.module_variables,
            event_statements: module.event_statements,
            comb_statements: module.comb_statements,
            _binary: binary,
        }
    }

    pub fn eval_comb(&self, mask_cache: &mut MaskCache) {
        for x in &self.comb_statements {
            x.eval_step(mask_cache);
        }
    }

    pub fn dump_variables(&self) -> String {
        format!("{}", self.module_variables)
    }

    /// Returns (jit_count, total_count) of top-level statements across all events and comb.
    pub fn jit_stats(&self) -> (usize, usize) {
        let mut jit = 0;
        let mut total = 0;
        for stmts in self.event_statements.values() {
            for s in stmts {
                total += 1;
                if s.is_binary() {
                    jit += 1;
                }
            }
        }
        for s in &self.comb_statements {
            total += 1;
            if s.is_binary() {
                jit += 1;
            }
        }
        (jit, total)
    }
}

pub fn build_ir(ir: air::Ir, top: StrId, config: &Config) -> Option<Ir> {
    for x in &ir.components {
        if let air::Component::Module(x) = x
            && top == x.name
        {
            let mut context = context::Context {
                config: config.clone(),
                ..Default::default()
            };
            let proto: ProtoModule = Conv::conv(&mut context, x)?;
            let module = proto.instantiate();
            return Some(Ir::from_module(module, context.binary));
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub use_4state: bool,
    pub use_jit: bool,
    pub dump_cranelift: bool,
    pub dump_asm: bool,
}

impl Config {
    pub fn all() -> Vec<Config> {
        let mut ret = vec![];

        for use_4state in [false, true] {
            for use_jit in [false, true] {
                ret.push(Config {
                    use_4state,
                    use_jit,
                    ..Default::default()
                });
            }
        }

        ret
    }
}
