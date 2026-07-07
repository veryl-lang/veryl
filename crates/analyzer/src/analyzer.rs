use crate::analyzer_error::{AnalyzerError, ExceedLimitKind};
use crate::attribute_table;
use crate::comb_loop_detect;
use crate::conv::{Context, Conv};
use crate::generic_inference_table;
use crate::handlers::*;
use crate::ir::{Ir, IrResult};
use crate::msb_table;
use crate::namespace::Namespace;
use crate::reference_table;
use crate::resolved_type_table;
use crate::scope;
use crate::symbol::{DocComment, ProjectPropertyValueProperty, Symbol, SymbolKind};
use crate::symbol_table;
use crate::type_dag;
use std::collections::BTreeMap;
use veryl_metadata::{Build, Lint, Metadata, ProjectProperty};
use veryl_parser::doc_comment_table;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::Token;
use veryl_parser::veryl_walker::{Handler, VerylWalker};

pub struct AnalyzerPass1 {
    handlers: Pass1Handlers,
}

impl AnalyzerPass1 {
    pub fn new(
        build_opt: &Build,
        lint_opt: &Lint,
        is_dependency: bool,
        project_name: StrId,
    ) -> Self {
        AnalyzerPass1 {
            handlers: Pass1Handlers::new(build_opt, lint_opt, is_dependency, project_name),
        }
    }
}

impl VerylWalker for AnalyzerPass1 {
    fn get_handlers(&mut self) -> Option<Vec<&mut dyn Handler>> {
        Some(self.handlers.get_handlers())
    }
}

#[derive(Clone)]
pub struct Analyzer {
    project_name: String,
    build_opt: Build,
    lint_opt: Lint,
}

fn insert_namespace_symbol(name: &str, public: bool) -> StrId {
    let token = Token::from_external_text(name);
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

fn insert_project_property_symbols(
    prject_name: StrId,
    properties: &BTreeMap<String, ProjectProperty>,
) {
    let mut namespace = Namespace::new();
    namespace.push(prject_name);

    for (prop_name, prop_value) in properties {
        let token = Token::from_external_text(prop_name);
        let value_property = ProjectPropertyValueProperty::new(prop_value, token.into());
        let symbol = Symbol::new(
            &token,
            SymbolKind::ProjectProperty(value_property),
            &namespace,
            false,
            DocComment::default(),
        );
        symbol_table::insert(&token, symbol);
    }
}

impl Analyzer {
    pub fn new(metadata: &Metadata) -> Self {
        let prj = insert_namespace_symbol(&metadata.project.name, true);
        insert_project_property_symbols(prj, &metadata.properties);

        for locks in metadata.lockfile.lock_table.values() {
            for lock in locks {
                let prj = insert_namespace_symbol(&lock.name, lock.visible);
                insert_project_property_symbols(prj, &lock.properties);

                for lock_dep in &lock.dependencies {
                    let from = resource_table::insert_str(&lock_dep.name);
                    let to = metadata
                        .lockfile
                        .lock_table
                        .get(&lock_dep.source.to_url())
                        .unwrap();

                    let to = to.iter().find(|x| x.source == lock_dep.source).unwrap();
                    let to_prj = insert_namespace_symbol(&to.name, to.visible);
                    insert_project_property_symbols(to_prj, &to.properties);

                    symbol_table::add_project_local(prj, from, to_prj);
                }
            }
        }
        Analyzer {
            project_name: metadata.project.name.clone(),
            build_opt: metadata.build.clone(),
            lint_opt: metadata.lint.clone(),
        }
    }

    pub fn analyze_pass1(&self, project_name: &str, input: &Veryl) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        let is_dependency = project_name != self.project_name;
        let project_name_id: StrId = project_name.into();
        scope::set_project(project_name_id, !is_dependency);
        let mut pass1 = AnalyzerPass1::new(
            &self.build_opt,
            &self.lint_opt,
            is_dependency,
            project_name_id,
        );
        pass1.veryl(input);
        ret.append(&mut pass1.handlers.get_errors());

        ret
    }

    pub fn analyze_post_pass1() -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        symbol_table::apply_import();
        symbol_table::resolve_user_defined();
        symbol_table::resolve_function();
        ret.append(&mut symbol_table::resolve_enum());
        ret.append(&mut symbol_table::apply_bind());
        ret.append(&mut symbol_table::apply_msb());
        ret.append(&mut symbol_table::apply_connect());
        generic_inference_table::resolve_pending();
        ret.append(&mut reference_table::apply());
        ret.append(&mut type_dag::apply());

        ret
    }

    fn create_ir(context: &mut Context, input: &Veryl) -> (Ir, Vec<AnalyzerError>) {
        let ir: IrResult<Ir> = Conv::conv(context, input);

        let (ir, mut errors) = if let Ok(mut ir) = ir {
            ir.eval_assign(context);
            let errors = context.drain_errors();
            (ir, errors)
        } else {
            let errors = context.drain_errors();
            (Ir::default(), errors)
        };

        // Surface a function-eval recursion-limit hit (#2891) that the
        // error-tolerant evaluation path would otherwise swallow. It is reported
        // as `ExceedLimit`, not `infinite_recursion`, since a finite-but-deep
        // recursion is a depth-limit hit rather than a true cycle.
        if let Some((token, depth)) = context.function_eval_overflow.take() {
            errors.push(AnalyzerError::exceed_limit(
                ExceedLimitKind::HierarchyDepth,
                depth,
                &token,
            ));
        }

        (ir, errors)
    }

    pub fn analyze_pass2(
        &self,
        input: &Veryl,
        context: &mut Context,
        ir: Option<&mut Ir>,
    ) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        context.config.retain_component_body = ir.is_some();
        context.config.instance_depth_limit = self.build_opt.instance_depth_limit;
        context.config.instance_total_limit = self.build_opt.instance_total_limit;
        context.config.function_instance_depth_limit = self.build_opt.function_instance_depth_limit;
        context.config.evaluate_size_limit = self.build_opt.evaluate_size_limit;
        context.config.evaluate_array_limit = self.build_opt.evaluate_array_limit;

        let mut ir_result = Self::create_ir(context, input);
        if let Some(x) = ir {
            x.append(&mut ir_result.0);
        }
        ret.append(&mut ir_result.1);

        ret
    }

    pub fn analyze_post_pass2(ir: &Ir) -> Vec<AnalyzerError> {
        let mut ret = Vec::new();

        ret.append(&mut symbol_table::check_unused_variable());
        ret.append(&mut symbol_table::check_wavedrom());
        ret.append(&mut comb_loop_detect::check(ir));

        ret
    }

    pub fn clear(&self) {
        attribute_table::clear();
        msb_table::clear();
        // `symbol_table::clear` also resets the scope arena (it re-registers
        // builtins that intern their scopes), keeping the two tables in sync.
        symbol_table::clear();
        type_dag::clear();
        resolved_type_table::clear();
        generic_inference_table::clear();
        doc_comment_table::clear();
    }
}
