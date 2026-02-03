use crate::analyzer_error::AnalyzerError;
use crate::attribute_table;
use crate::conv::{Context, Conv};
use crate::handlers::*;
use crate::ir::{Ir, IrResult};
use crate::msb_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::reference_table;
use crate::symbol::{DocComment, Symbol, SymbolKind};
use crate::symbol_table;
use crate::type_dag;
use veryl_metadata::{Build, Lint, Metadata};
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, TokenSource};
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1 {
    handlers: Pass1Handlers,
}

impl AnalyzerPass1 {
    pub fn new(build_opt: &Build, lint_opt: &Lint) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(build_opt, lint_opt),
        }
    }
}

impl VerylWalker for AnalyzerPass1 {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

pub struct Analyzer {
    build_opt: Build,
    lint_opt: Lint,
}

fn insert_namespace_symbol(name: &str, public: bool) -> StrId {
    let token = Token::new(name, 0, 0, 0, 0, TokenSource::External);
    let symbol = Symbol::new(
        &token,
        SymbolKind::Namespace,
        &Namespace::new(),
        public,
        DocComment::default(),
    );
    symbol_table::insert(&token, symbol);
    token.text
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        insert_namespace_symbol(&metadata.project.name, true);
        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = insert_namespace_symbol(&lock.name, lock.visible);
                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata
                        .lockfile
                        .lock_table
                        .get(&lock_dep.source.to_url())
                        .unwrap();

                    let to = to.iter().find(|x| x.source == lock_dep.source).unwrap();
                    let to = insert_namespace_symbol(&to.name, to.visible);
                    symbol_table::add_project_local(prj, from, to);
                }
            }
        }
        Analyzer {
            build_opt: metadata.build.clone(),
            lint_opt: metadata.lint.clone(),
        }
    }

    pub fn analyze_pass1(&self, project_name: &str, input: &Veryl) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        namespace_table::set_default(&[project_name.into()]);
        let mut pass1 = AnalyzerPass1::new(&self.build_opt, &self.lint_opt);
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
    }

    pub fn analyze_post_pass1() -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        symbol_table::apply_import();
        symbol_table::resolve_user_defined();
        ret.append(&mut symbol_table::resolve_enum());
        ret.append(&mut symbol_table::apply_bind());
        ret.append(&mut symbol_table::apply_msb());
        ret.append(&mut symbol_table::apply_connect());
        ret.append(&mut reference_table::apply());
        ret.append(&mut type_dag::apply());

        ret
    }

    fn create_ir(context: &mut Context, input: &Veryl) -> (Ir, Vec<AnalyzerError>) {
        let ir: IrResult<Ir> = Conv::conv(context, input);
        context.insert_ir_error(&ir);

        if let Ok(ir) = ir {
            ir.eval_assign(context);
            let errors = context.drain_errors();
            (ir, errors)
        } else {
            let errors = context.drain_errors();
            (Ir::default(), errors)
        }
    }

    pub fn analyze_pass2(
        &self,
        project_name: &str,
        input: &Veryl,
        context: &mut Context,
        ir: Option<&mut Ir>,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        context.config.use_ir = ir.is_some();
        context.config.instance_depth_limit = self.build_opt.instance_depth_limit;
        context.config.instance_total_limit = self.build_opt.instance_total_limit;
        context.config.evaluate_size_limit = self.build_opt.evaluate_size_limit;
        context.config.evaluate_array_limit = self.build_opt.evaluate_array_limit;

        namespace_table::set_default(&[project_name.into()]);
        let mut ir_result = Self::create_ir(context, input);
        if let Some(x) = ir {
            x.append(&mut ir_result.0);
        }
        ret.append(&mut ir_result.1);

        ret
    }

    pub fn analyze_post_pass2() -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        ret.append(&mut symbol_table::check_unused_variable());

        ret
    }

    pub fn clear(&self) {
        attribute_table::clear();
        msb_table::clear();
        namespace_table::clear();
        symbol_table::clear();
        type_dag::clear();
    }
}
