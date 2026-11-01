use crate::analyzer_error::{AnalyzerError, ExceedLimitKind, InstanceNote};
use crate::conv::conv_profiler::{ConvProfile, ConvProfileGuard};
use crate::conv::instance::{InstanceHistory, InstanceHistoryError, Reach};
use crate::conv::package_const::{PackageConstCache, PackageConstKey, PackageConstUse};
use crate::conv::unit_cache::{self, ChildReach, UnitEvent};
use crate::ir::{
    Component, Comptime, Declaration, Expression, FfClock, FfReset, FuncPath, Function, Interface,
    IrResult, ShapeRef, Signature, Statement, Type, VarId, VarIndex, VarKind, VarPath, VarSelect,
    Variable, VariableInfo,
};
use crate::namespace::Namespace;
use crate::scope;
use crate::symbol::{Affiliation, ClockDomain, Direction, GenericMap, Symbol, SymbolId};
use crate::symbol_path::GenericSymbolPath;
use crate::value::MaskCache;
use crate::{HashMap, HashSet};
use miette::Result;
use std::sync::{Arc, Mutex};
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::{ComponentInstantiation, Identifier};
use veryl_parser::veryl_token::Token;

#[derive(Clone)]
pub struct Config {
    pub retain_component_body: bool,
    pub instance_depth_limit: usize,
    pub instance_total_limit: usize,
    pub function_instance_depth_limit: usize,
    /// Recursion bound for `eval_factor_path`. Distinct from the function
    /// limit above: a `const` chain costs one level per link and is not a
    /// function instantiation at all.
    pub symbol_eval_depth_limit: usize,
    pub evaluate_size_limit: usize,
    pub evaluate_array_limit: usize,
    pub defines: HashSet<StrId>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeFunctionEffect {
    pub external_write: bool,
    pub written_outputs: HashSet<VarPath>,
}

impl RuntimeFunctionEffect {
    pub fn written_paths_for<'a>(
        &'a self,
        formal: &'a VarPath,
    ) -> impl Iterator<Item = &'a VarPath> {
        self.written_outputs
            .iter()
            .filter(move |path| path.0.starts_with(&formal.0))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionOutputBinding {
    pub scheduler_external: bool,
}

#[derive(Clone, Debug)]
struct FunctionEffectFrame {
    path: FuncPath,
    outputs: HashMap<VarId, FunctionOutputBinding>,
    effect: RuntimeFunctionEffect,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            retain_component_body: false,
            instance_depth_limit: 1024,
            instance_total_limit: 1024 * 1024,
            function_instance_depth_limit: 24,
            symbol_eval_depth_limit: 48,
            evaluate_size_limit: 1024 * 1024,
            evaluate_array_limit: 128,
            defines: HashSet::default(),
        }
    }
}

#[derive(Default)]
pub struct Context {
    pub config: Config,
    /// Constant procedural ranges in scope while converting a retained body.
    /// Used to prove a condition uniform without duplicating that body.
    pub(crate) for_ranges: Vec<(VarId, crate::ir::ForRange)>,
    pub var_id: VarId,
    pub var_paths: HashMap<VarPath, (VarId, Comptime)>,
    pub func_paths: HashMap<FuncPath, VarId>,
    /// Symbols whose function bodies are mid-conversion. Carried (via `inherit`)
    /// across the fresh per-call contexts that mutual recursion creates, so
    /// re-entry is detected and the IR conversion doesn't overflow.
    pub converting_funcs: Vec<SymbolId>,
    pub variables: HashMap<VarId, Variable>,
    pub functions: HashMap<VarId, Function>,
    pub(crate) function_value_cache: crate::ir::FunctionValueCache,
    pub port_types: HashMap<VarPath, (Type, ClockDomain)>,
    pub modports: HashMap<StrId, Vec<(StrId, Direction)>>,
    pub declarations: Vec<Declaration>,
    pub default_clock: Option<(VarPath, SymbolId)>,
    pub default_reset: Option<(VarPath, SymbolId)>,
    pub inst_signatures: HashMap<StrId, Signature>,
    pub modport_signatures: Vec<HashMap<StrId, Signature>>,
    pub instance_history: InstanceHistory,
    package_const_cache: PackageConstCache,
    /// Recursion depth of `eval_factor_path`, bounded by `symbol_eval_depth_limit`.
    pub function_eval_depth: usize,
    pub function_eval_peak: usize,
    /// Recorded when `function_eval_depth`'s limit is hit, since the eval path
    /// may swallow an inserted error.
    pub function_eval_overflow: Option<(TokenRange, usize)>,
    /// Recorded when a comptime for loop exceeds evaluate_size_limit; skipping
    /// its body would silently fold consts to their pre-loop values.
    pub comptime_for_overflow: Option<TokenRange>,
    /// Active generic-function call signatures, to detect exact re-entry as
    /// infinite recursion. Not `instance_history`: that only marks a signature
    /// complete via `set_instance_history`, which functions never call, so it
    /// would stay wedged as "active" forever.
    pub function_call_stack: Vec<Signature>,
    function_effects: HashMap<FuncPath, RuntimeFunctionEffect>,
    function_effect_stack: Vec<FunctionEffectFrame>,
    /// Return variable of the function currently being constant-evaluated, and
    /// whether one of its `return` statements has already run. A `return` is
    /// lowered to an assignment to that variable, so without this the
    /// evaluator walks straight past it and the LAST `return` wins.
    pub function_ret_var: Option<VarId>,
    pub function_returned: bool,
    /// Set when the function being constant-evaluated reached a branch or
    /// loop whose condition or bound is unknown, or a write it could not
    /// perform. Skipping that code leaves stale values behind, so the call
    /// result is unknown too.
    pub function_indeterminate: bool,
    pub select_paths: Vec<(VarPath, GenericSymbolPath)>,
    pub select_dims: Vec<usize>,
    pub ignore_var_func: bool,
    pub disalbe_const_opt: bool,
    pub namespaces: Vec<Namespace>,
    pub in_generic: bool,
    pub allow_component_as_factor: bool,
    /// Depth of `size_in_component_scope`.
    component_sizing: usize,
    global_func_callers: Vec<Namespace>,
    pub in_test_module: bool,
    /// In a test module and the components its hierarchical references may
    /// reach: the identifiers of the test module, which name every instance
    /// such a reference passes. A component instantiated under one of them is
    /// read whole, so a replayed one is converted whole.
    pub body_names: Option<Arc<HashSet<StrId>>>,
    pub in_dependency: bool,
    pub in_global_func: Option<Token>,
    pub in_if_reset: bool,
    /// Inside an initial/final block (testbench statement context).
    pub in_tb_block: bool,
    /// Inside an `initial` block, holding the next `VarId` at its entry so the
    /// block's own procedural locals can be told from the module's state.
    /// `in_tb_block` is unusable here: it also covers `final` and connections.
    pub in_initial: Option<VarId>,
    /// Sink for component method calls hoisted out of the current
    /// testbench statement's expressions; `Some` only while initial/final
    /// statements convert. Each hoisted call runs as its own zero-time
    /// statement immediately before the enclosing one.
    pub tb_hoist: Option<Vec<crate::ir::Statement>>,
    /// Names hoist temporaries uniquely within a module.
    pub tb_hoist_count: usize,
    /// Resolved numeric parameters of the external component instances
    /// declared so far, for evaluating declared method width expressions.
    pub tb_component_params:
        crate::HashMap<veryl_parser::resource_table::StrId, Vec<(String, u64)>>,
    /// Elaborated components of module instances declared so far in the
    /// current module, for hierarchical references from testbench code.
    /// Child module bodies convert in fresh contexts (`inherit` does not
    /// carry this map), so it only holds the current module's instances.
    pub inst_components: HashMap<StrId, Arc<Component>>,
    /// Inside an inst input-port-connection conversion; consumed by
    /// `check_compatibility` to pick the clock/reset diagnostic.
    pub in_inst_port: bool,
    pub current_clock: Option<Comptime>,
    /// Comptimes of the enclosing if/case/switch statement conditions: a
    /// condition gates the writes below it like a mux select, so its clock
    /// domain is checked against each assignment destination.
    pub condition_domains: Vec<Comptime>,
    pub mask_cache: MaskCache,
    pub tb_reset_cycles: HashMap<StrId, Expression>,
    pub tb_clock_period: HashMap<StrId, Expression>,
    pub tb_reset_clock: HashMap<StrId, StrId>,
    /// Sparse rollback state used by analyses that reuse this otherwise-large
    /// context across independent procedures. Empty during normal conversion.
    analysis_transactions: Vec<AnalysisTransaction>,
    hierarchy: Vec<StrId>,
    hierarchical_variables: Vec<Vec<VarPath>>,
    hierarchical_functions: Vec<Vec<FuncPath>>,
    shadowed_variables: HashMap<VarPath, Vec<(VarId, Comptime)>>,
    affiliation: Vec<Affiliation>,
    overrides: Vec<ParameterOverride>,
    generic_maps: Vec<Vec<GenericMap>>,
    /// Typedefs currently being expanded into an IR type, to break cyclic
    /// `type A = B; type B = A` chains that `eval_type` would otherwise follow
    /// forever (type_dag reports the cycle, but `dump` evaluates anyway).
    typedef_visiting: Vec<SymbolId>,
    errors: ErrorSink,
    profiler: Option<Arc<Mutex<ConvProfile>>>,
    project_name: Option<String>,
}

/// The diagnostics of a conversion. Moves with them through `inherit`, so the
/// recording and the suppression cover everything converted in a unit.
#[derive(Default)]
struct ErrorSink {
    errors: Vec<AnalyzerError>,
    /// What a recorded unit did to its own diagnostics.
    events: Option<Vec<UnitEvent>>,
    eval_overflow_recorded: bool,
    for_overflow_recorded: bool,
    /// Set while converting a component whose diagnostics were replayed.
    suppressed: bool,
}

/// Namespace-tagged so a same-named parameter of another component is not
/// answered from this frame.
#[derive(Clone)]
struct ParameterOverride {
    namespace: Namespace,
    values: HashMap<VarPath, (Comptime, Expression)>,
}

struct AnalysisTransaction {
    variables: HashMap<VarId, Variable>,
    comptime_for_overflow: Option<TokenRange>,
    disable_const_opt: bool,
    function_eval_depth: usize,
    function_eval_overflow: Option<(TokenRange, usize)>,
    function_call_stack: Vec<Signature>,
    errors: ErrorSink,
}

impl Context {
    pub(crate) fn begin_analysis_transaction(&mut self) {
        self.analysis_transactions.push(AnalysisTransaction {
            variables: HashMap::default(),
            comptime_for_overflow: self.comptime_for_overflow,
            disable_const_opt: self.disalbe_const_opt,
            function_eval_depth: self.function_eval_depth,
            function_eval_overflow: self.function_eval_overflow.take(),
            function_call_stack: std::mem::take(&mut self.function_call_stack),
            errors: std::mem::take(&mut self.errors),
        });
        self.function_eval_depth = 0;
        self.comptime_for_overflow = None;
    }

    pub(crate) fn rollback_analysis_transaction(&mut self) {
        let transaction = self
            .analysis_transactions
            .pop()
            .expect("analysis transaction is active");
        for (id, variable) in transaction.variables {
            self.variables.insert(id, variable);
        }
        self.comptime_for_overflow = transaction.comptime_for_overflow;
        self.disalbe_const_opt = transaction.disable_const_opt;
        self.function_eval_depth = transaction.function_eval_depth;
        self.function_eval_overflow = transaction.function_eval_overflow;
        self.function_call_stack = transaction.function_call_stack;
        self.errors = transaction.errors;
    }

    /// Get a variable for mutation while remembering its prior value in the
    /// innermost reusable-analysis transaction. Outside such an analysis this
    /// is identical to `variables.get_mut`.
    pub(crate) fn variable_mut(&mut self, id: &VarId) -> Option<&mut Variable> {
        if self
            .analysis_transactions
            .last()
            .is_some_and(|transaction| !transaction.variables.contains_key(id))
        {
            let previous = self.variables.get(id)?.clone();
            self.analysis_transactions
                .last_mut()
                .expect("checked above")
                .variables
                .insert(*id, previous);
        }
        self.variables.get_mut(id)
    }

    /// Run `f` with `cond` on the statement-condition stack (popped before
    /// `?` can skip it).
    pub fn with_condition_domain<T>(
        &mut self,
        cond: Comptime,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.condition_domains.push(cond);
        let ret = f(self);
        self.condition_domains.pop();
        ret
    }

    pub fn inherit(&mut self, tgt: &mut Context) {
        std::mem::swap(&mut self.overrides, &mut tgt.overrides);
        std::mem::swap(&mut self.generic_maps, &mut tgt.generic_maps);
        std::mem::swap(&mut self.modport_signatures, &mut tgt.modport_signatures);
        std::mem::swap(&mut self.instance_history, &mut tgt.instance_history);
        std::mem::swap(&mut self.package_const_cache, &mut tgt.package_const_cache);
        std::mem::swap(&mut self.function_eval_depth, &mut tgt.function_eval_depth);
        std::mem::swap(&mut self.function_eval_peak, &mut tgt.function_eval_peak);
        std::mem::swap(
            &mut self.function_eval_overflow,
            &mut tgt.function_eval_overflow,
        );
        std::mem::swap(
            &mut self.comptime_for_overflow,
            &mut tgt.comptime_for_overflow,
        );
        std::mem::swap(&mut self.function_call_stack, &mut tgt.function_call_stack);
        std::mem::swap(&mut self.function_effects, &mut tgt.function_effects);
        std::mem::swap(
            &mut self.function_effect_stack,
            &mut tgt.function_effect_stack,
        );
        std::mem::swap(&mut self.converting_funcs, &mut tgt.converting_funcs);
        std::mem::swap(&mut self.errors, &mut tgt.errors);
        std::mem::swap(&mut self.namespaces, &mut tgt.namespaces);
        std::mem::swap(&mut self.global_func_callers, &mut tgt.global_func_callers);
        self.disalbe_const_opt = tgt.disalbe_const_opt;
        self.in_generic = tgt.in_generic;
        self.allow_component_as_factor = tgt.allow_component_as_factor;
        self.component_sizing = tgt.component_sizing;
        self.body_names = tgt.body_names.clone();
        self.config = tgt.config.clone();
        self.profiler = tgt.profiler.clone();
        self.project_name = tgt.project_name.clone();
    }

    /// Runs `f` on a context carrying only the inputs a component conversion
    /// may depend on: the frames pushed for `namespace` and the shared caches.
    /// Its diagnostics are merged back as raised, keeping their instance notes.
    ///
    /// With `record`, what the unit does to its diagnostics is returned for
    /// the unit cache. With `suppressed`, its diagnostics are dropped: they
    /// were replayed from that cache.
    pub fn in_unit<T>(
        &mut self,
        namespace: &Namespace,
        instantiated: bool,
        record: bool,
        suppressed: bool,
        body_names: Option<Arc<HashSet<StrId>>>,
        f: impl FnOnce(&mut Context) -> T,
    ) -> (T, Option<Vec<UnitEvent>>) {
        let mut unit = Context {
            config: self.config.clone(),
            profiler: self.profiler.clone(),
            project_name: self.project_name.clone(),
            in_generic: self.in_generic,
            body_names,
            ..Default::default()
        };
        if let Some(x) = self
            .overrides
            .iter()
            .rev()
            .find(|x| x.namespace == *namespace)
        {
            unit.overrides.push(x.clone());
        }
        if let Some(x) = self.generic_maps.last() {
            unit.generic_maps.push(x.clone());
        }
        if instantiated && let Some(x) = self.modport_signatures.last() {
            unit.modport_signatures.push(x.clone());
        }
        let suppressed = suppressed || self.errors.suppressed;
        unit.errors.suppressed = suppressed;
        if record && !suppressed {
            unit.errors.events = Some(Vec::new());
        }
        self.swap_unit_caches(&mut unit);

        let ret = f(&mut unit);

        self.swap_unit_caches(&mut unit);
        unit.record_overflow();
        if !suppressed {
            if self.function_eval_overflow.is_none() {
                self.function_eval_overflow = unit.function_eval_overflow;
            }
            if self.comptime_for_overflow.is_none() {
                self.comptime_for_overflow = unit.comptime_for_overflow;
            }
            for mut error in unit.errors.errors {
                let note = error.instance_mut().take();
                self.insert_error_with_note(error, note);
            }
        }
        (ret, unit.errors.events)
    }

    /// Records the first overflow of a unit when it is first seen, which
    /// places it before the children reached later.
    fn record_overflow(&mut self) {
        let sink = &mut self.errors;
        let Some(events) = &mut sink.events else {
            return;
        };
        if let Some((token, depth)) = self.function_eval_overflow
            && !sink.eval_overflow_recorded
        {
            events.push(UnitEvent::EvalOverflow(token, depth));
            sink.eval_overflow_recorded = true;
        }
        if let Some(token) = self.comptime_for_overflow
            && !sink.for_overflow_recorded
        {
            events.push(UnitEvent::ForOverflow(token));
            sink.for_overflow_recorded = true;
        }
    }

    pub fn record_child_reach(&mut self, reach: ChildReach) {
        self.record_overflow();
        if let Some(events) = &mut self.errors.events {
            events.push(UnitEvent::Child(reach));
        }
    }

    /// The `body_names` of the component instantiated at `site`, when a
    /// hierarchical reference may read its body.
    pub fn body_names_at(&self, site: Option<TokenRange>) -> Option<Arc<HashSet<StrId>>> {
        let names = self.body_names.as_ref()?;
        names.contains(&site?.beg.text).then(|| Arc::clone(names))
    }

    /// Applies a replayed event of a unit whose conversion was skipped. The
    /// caller replays children itself.
    pub fn replay_event(&mut self, event: &UnitEvent) {
        // A reference is not a diagnostic: it stands whether or not they are
        // suppressed.
        if let UnitEvent::Reference(target, token) = event {
            let path = crate::symbol_path::SymbolPath::new(&[target.text]);
            if let Some((scope, define_context)) = scope::token_scope(target.id)
                && let Ok(symbol) = crate::symbol_table::resolve(
                    crate::symbol_path::SymbolPathNamespace::from_scope(
                        path,
                        scope,
                        define_context,
                    ),
                )
            {
                crate::symbol_table::add_reference(symbol.found.id, token);
            }
        }
        if self.errors.suppressed {
            return;
        }
        match event {
            UnitEvent::Error(x) => {
                let note = self.instance_note();
                self.insert_error_with_note(unit_cache::restore_file_text(x.clone()), note);
            }
            UnitEvent::EvalOverflow(token, depth) => {
                if self.function_eval_overflow.is_none() {
                    self.function_eval_overflow = Some((*token, *depth));
                }
            }
            UnitEvent::ForOverflow(token) => {
                if self.comptime_for_overflow.is_none() {
                    self.comptime_for_overflow = Some(*token);
                }
            }
            UnitEvent::ResolvedType(token, inferred) => {
                // A conversion would check against the type entered already,
                // and report a conflict: convert the units instead.
                let chain = self.instance_history.keys();
                match crate::resolved_type_table::get(token) {
                    Some(existing) => {
                        if crate::ir::stable_type(&existing) != *inferred {
                            unit_cache::request_conversion(&chain);
                        }
                    }
                    None => unit_cache::note_replayed_type(*token, inferred.clone(), chain),
                }
            }
            UnitEvent::Reference(..) => (),
            UnitEvent::Child(_) => (),
        }
    }

    pub fn add_reference(&mut self, id: SymbolId, token: Token) {
        crate::symbol_table::add_reference(id, &token);
        if let Some(events) = &mut self.errors.events
            && let Some(symbol) = crate::symbol_table::get(id)
        {
            events.push(UnitEvent::Reference(symbol.token, token));
        }
    }

    pub fn record_resolved_type(
        &mut self,
        token: veryl_parser::resource_table::TokenId,
        inferred: String,
    ) {
        if let Some(events) = &mut self.errors.events {
            events.push(UnitEvent::ResolvedType(token, inferred));
        }
    }

    fn swap_unit_caches(&mut self, unit: &mut Context) {
        std::mem::swap(&mut self.instance_history, &mut unit.instance_history);
        std::mem::swap(&mut self.package_const_cache, &mut unit.package_const_cache);
        std::mem::swap(&mut self.function_effects, &mut unit.function_effects);
    }

    pub fn set_project_name(&mut self, project_name: &str) {
        self.project_name = Some(project_name.to_string());
    }

    pub fn project_name(&self) -> Option<&str> {
        self.project_name.as_deref()
    }

    pub fn enable_conv_profiler(&mut self) {
        if std::env::var("VERYL_CONV_PROFILE").is_ok() {
            self.profiler = Some(Arc::new(Mutex::new(ConvProfile::default())));
        }
    }

    pub fn conv_profile_guard(&self, component_name: StrId) -> Option<ConvProfileGuard> {
        self.profiler.as_ref().map(|p| {
            let key = format!(
                "{}::{}",
                self.project_name.as_deref().unwrap_or("unknown"),
                component_name
            );
            ConvProfileGuard::new(Arc::clone(p), key)
        })
    }

    pub fn finalize_conv_profiler(&self) -> Result<()> {
        if let Some(profiler) = self.profiler.as_ref().map(|p| p.lock().unwrap()) {
            profiler.to_toml_file(&std::env::var("VERYL_CONV_PROFILE").unwrap())
        } else {
            Ok(())
        }
    }

    /// Innermost-first, so a component instantiated inside another
    /// instantiation of itself reads its own values. `matched` rather than
    /// `==`: a parameter under an `#[ifdef]` carries that define context while
    /// the frame carries the component's, and the two name one scope unless the
    /// contexts exclude each other.
    pub fn get_override(
        &self,
        namespace: &Namespace,
        x: &VarPath,
    ) -> Option<&(Comptime, Expression)> {
        self.overrides
            .iter()
            .rev()
            .find(|o| o.namespace.matched(namespace))
            .and_then(|o| o.values.get(x))
    }

    pub fn get_variable_info(&self, id: VarId) -> Option<VariableInfo> {
        self.variables.get(&id).map(VariableInfo::new)
    }

    pub(crate) fn begin_function_effect(
        &mut self,
        path: FuncPath,
        outputs: HashMap<VarId, FunctionOutputBinding>,
        effect: RuntimeFunctionEffect,
    ) {
        self.function_effect_stack.push(FunctionEffectFrame {
            path,
            outputs,
            effect,
        });
    }

    pub(crate) fn finish_function_effect(&mut self) {
        if let Some(frame) = self.function_effect_stack.pop() {
            self.function_effects.insert(frame.path, frame.effect);
        }
    }

    pub(crate) fn function_effect(&self, path: &FuncPath) -> Option<RuntimeFunctionEffect> {
        self.function_effects.get(path).cloned()
    }

    pub(crate) fn record_function_call_effect(
        &mut self,
        effect: &RuntimeFunctionEffect,
        outputs: &[(VarPath, Vec<crate::ir::AssignDestination>)],
    ) {
        let Some(frame) = self.function_effect_stack.last() else {
            return;
        };
        let caller_outputs = frame.outputs.clone();
        let mut external_write = effect.external_write;
        let mut written_outputs = HashSet::default();

        for (formal, destinations) in outputs {
            for written in effect.written_paths_for(formal) {
                let suffix = &written.0[formal.0.len()..];
                for destination in destinations {
                    if let Some(output) = caller_outputs.get(&destination.id) {
                        let mut mapped = destination.path.clone();
                        mapped.0.extend_from_slice(suffix);
                        written_outputs.insert(mapped);
                        external_write |= output.scheduler_external;
                    } else if self
                        .get_variable_info(destination.id)
                        .is_none_or(|variable| variable.affiliation != Affiliation::Function)
                    {
                        external_write = true;
                    }
                }
            }
        }

        if let Some(frame) = self.function_effect_stack.last_mut() {
            frame.effect.external_write |= external_write;
            frame.effect.written_outputs.extend(written_outputs);
        }
    }

    pub(crate) fn record_function_statement_writes(&mut self, statements: &[Statement]) {
        let Some(caller_outputs) = self
            .function_effect_stack
            .last()
            .map(|frame| frame.outputs.clone())
        else {
            return;
        };
        let mut effect = RuntimeFunctionEffect::default();

        fn visit(
            context: &Context,
            statements: &[Statement],
            caller_outputs: &HashMap<VarId, FunctionOutputBinding>,
            effect: &mut RuntimeFunctionEffect,
        ) {
            for statement in statements {
                match statement {
                    Statement::Assign(assign) => {
                        for destination in &assign.dst {
                            if let Some(output) = caller_outputs.get(&destination.id) {
                                effect.written_outputs.insert(destination.path.clone());
                                effect.external_write |= output.scheduler_external;
                            } else if context.get_variable_info(destination.id).is_none_or(
                                |variable| {
                                    variable.affiliation != Affiliation::Function
                                        || matches!(variable.kind, VarKind::Output | VarKind::Inout)
                                },
                            ) {
                                effect.external_write = true;
                            }
                        }
                    }
                    Statement::If(statement) => {
                        visit(context, &statement.true_side, caller_outputs, effect);
                        visit(context, &statement.false_side, caller_outputs, effect);
                    }
                    Statement::IfReset(statement) => {
                        visit(context, &statement.true_side, caller_outputs, effect);
                        visit(context, &statement.false_side, caller_outputs, effect);
                    }
                    Statement::Case(statement) => {
                        for arm in &statement.arms {
                            visit(context, &arm.body, caller_outputs, effect);
                        }
                        visit(context, &statement.default, caller_outputs, effect);
                    }
                    Statement::For(statement) => {
                        visit(context, &statement.body, caller_outputs, effect);
                    }
                    Statement::SystemFunctionCall(_)
                    | Statement::FunctionCall(_)
                    | Statement::TbMethodCall(_)
                    | Statement::Break
                    | Statement::Unsupported(_)
                    | Statement::Null => {}
                }
            }
        }

        visit(self, statements, &caller_outputs, &mut effect);
        if let Some(frame) = self.function_effect_stack.last_mut() {
            frame.effect.external_write |= effect.external_write;
            frame.effect.written_outputs.extend(effect.written_outputs);
        }
    }

    pub fn resolve_path(&self, path: GenericSymbolPath) -> GenericSymbolPath {
        self.resolve_path_mapped(path).0
    }

    /// Like `resolve_path`, also telling whether a generic map replaced the
    /// head. A replaced name must not be looked up as a variable: in an inlined
    /// function body it would find a same-named variable of the caller.
    pub fn resolve_path_mapped(&self, mut path: GenericSymbolPath) -> (GenericSymbolPath, bool) {
        let Some((scope, define_context)) = scope::token_scope(path.paths[0].base.id) else {
            return (path, false);
        };
        path.resolve_imported(scope, &define_context, None);
        let mapped = self.apply_generic_maps(&mut path);
        (path, mapped)
    }

    /// Returns whether a generic map replaced the head of `path`.
    pub fn apply_generic_maps(&self, path: &mut GenericSymbolPath) -> bool {
        let mut mapped = false;
        for map in self.generic_maps.iter().rev() {
            let head = path.paths[0].base();
            mapped |= map.iter().any(|x| x.map.contains_key(&head));
            path.apply_map(map);
        }
        mapped
    }

    pub fn insert_var_path(&mut self, path: VarPath, value: Comptime) -> VarId {
        let id = self.var_id;
        self.insert_var_path_with_id(path, id, value);
        self.var_id.inc();
        id
    }

    pub fn insert_var_path_with_id(&mut self, path: VarPath, id: VarId, value: Comptime) {
        if let Some(x) = self.hierarchical_variables.last_mut() {
            x.push(path.clone());
        }

        let shadowed = self.var_paths.insert(path.clone(), (id, value));

        // store variable shadowed by inner-scope variable
        if let Some(x) = shadowed {
            self.shadowed_variables
                .entry(path)
                .and_modify(|v| v.push(x.clone()))
                .or_insert(vec![x]);
        }
    }

    pub fn insert_func_path(&mut self, path: FuncPath) -> VarId {
        let id = self.var_id;
        self.func_paths.insert(path, id);
        self.var_id.inc();
        id
    }

    pub fn insert_variable(&mut self, id: VarId, mut variable: Variable) {
        if self.ignore_var_func {
            return;
        }

        let hier = &self.hierarchy;
        if !hier.is_empty() {
            variable.path.add_prelude(hier);
        }
        self.variables.insert(id, variable);
    }

    pub fn insert_port_type(&mut self, path: VarPath, r#type: Type, clock_domain: ClockDomain) {
        self.port_types.insert(path, (r#type, clock_domain));
    }

    pub fn insert_function(&mut self, id: VarId, mut function: Function) {
        if self.ignore_var_func {
            return;
        }

        let hier = &self.hierarchy;
        if !hier.is_empty() {
            function.path.add_prelude(hier);
        }
        self.functions.insert(id, function);
    }

    pub fn insert_declaration(&mut self, decl: Declaration) {
        if !decl.is_null() {
            self.declarations.push(decl);
        }
    }

    pub fn insert_error(&mut self, mut error: AnalyzerError) {
        if self.errors.suppressed {
            return;
        }
        if let Some(events) = &mut self.errors.events {
            events.push(UnitEvent::Error(unit_cache::record_copy(&mut error)));
        }
        let note = self.instance_note();
        self.insert_error_with_note(error, note);
    }

    pub fn insert_error_unrecorded(&mut self, error: AnalyzerError) {
        if self.errors.suppressed {
            return;
        }
        let note = self.instance_note();
        self.insert_error_with_note(error, note);
    }

    fn insert_error_with_note(&mut self, mut error: AnalyzerError, note: Option<InstanceNote>) {
        let existing = self
            .errors
            .errors
            .iter_mut()
            .position(|x| x.same_report(&mut error));
        if let Some(existing) = existing {
            // Raised without an override too, so no instance explains it.
            if note.is_none() {
                *self.errors.errors[existing].instance_mut() = None;
            }
        } else {
            *error.instance_mut() = note;
            self.errors.errors.push(error);
        }
    }

    /// An evaluation that reports anything is not kept, and a kept one is not
    /// reused where redoing it would exceed the depth limit: either way the
    /// reuse would change what is reported.
    pub(crate) fn eval_package_const(
        &mut self,
        symbol: &Symbol,
        usage: PackageConstUse,
        eval: impl FnOnce(&mut Self) -> IrResult<Comptime>,
    ) -> IrResult<Comptime> {
        let key = PackageConstKey::new(symbol, usage, self.in_generic, &self.generic_maps);
        let start = self.function_eval_depth;
        if let Some((value, depth)) = key.as_ref().and_then(|x| self.package_const_cache.get(x))
            && start + depth <= self.config.symbol_eval_depth_limit
        {
            self.function_eval_peak = self.function_eval_peak.max(start + depth);
            return Ok(value.clone());
        }

        #[cfg(test)]
        PACKAGE_CONST_EVALUATIONS.set(PACKAGE_CONST_EVALUATIONS.get() + 1);
        let outer_peak = std::mem::replace(&mut self.function_eval_peak, start);
        let outer_eval_overflow = self.function_eval_overflow.take();
        let outer_for_overflow = self.comptime_for_overflow.take();
        let outer_errors = std::mem::take(&mut self.errors);
        let ret = eval(self);
        let errors = std::mem::replace(&mut self.errors, outer_errors);

        let depth = self.function_eval_peak - start;
        self.function_eval_peak = self.function_eval_peak.max(outer_peak);
        let overflowed =
            self.function_eval_overflow.is_some() || self.comptime_for_overflow.is_some();
        // Only the first overflow is recorded.
        if outer_eval_overflow.is_some() {
            self.function_eval_overflow = outer_eval_overflow;
        }
        if outer_for_overflow.is_some() {
            self.comptime_for_overflow = outer_for_overflow;
        }

        if let (Some(key), Ok(value)) = (key, &ret)
            && errors.errors.is_empty()
            && !overflowed
        {
            self.package_const_cache.insert(key, value.clone(), depth);
        }
        for error in errors.errors {
            self.insert_error(error);
        }
        ret
    }

    fn instance_note(&self) -> Option<InstanceNote> {
        // Only where the values are written and where they arrive matter;
        // a recursive instantiation would otherwise list every level.
        const KEPT_AT_EACH_END: usize = 2;

        let frames = self.instance_history.overridden_frames();
        let (_, outermost_site) = frames.first()?;
        let mut path: Vec<_> = frames
            .iter()
            .map(|(sig, site)| format!("{}: {}", site.beg.text, sig))
            .collect();
        if path.len() > KEPT_AT_EACH_END * 2 + 1 {
            let elided = path.len() - KEPT_AT_EACH_END * 2;
            path.splice(
                KEPT_AT_EACH_END..path.len() - KEPT_AT_EACH_END,
                [format!("({elided} more)")],
            );
        }
        Some(InstanceNote::new(path.join(" -> "), outermost_site))
    }

    pub fn insert_modport(&mut self, name: StrId, members: Vec<(StrId, Direction)>) {
        self.modports.insert(name, members);
    }

    pub fn extract_function(&mut self, context: &mut Context, base: &VarPath, array: &ShapeRef) {
        for (id, mut variable) in context.variables.drain() {
            let path_offset = variable.path.0.len();
            variable.path.add_prelude(&base.0);
            variable.prepend_array_at_path(array, path_offset);
            self.variables.insert(id, variable);
        }

        for (mut path, id) in context.func_paths.drain() {
            if !path.path.starts_with(&base.0) {
                path.path.add_prelude(&base.0);
            }
            self.func_paths.insert(path, id);
        }

        for (id, mut function) in context.functions.drain() {
            if !array.is_empty() {
                let total_array = array.total();
                let func_body = function.functions.remove(0);
                if let Some(total_array) = total_array {
                    for i in 0..total_array {
                        let var_index = VarIndex::from_index(i, array);
                        let mut func_body = func_body.clone();
                        func_body.set_index(&var_index);
                        function.functions.push(func_body);
                    }
                }
            }

            if !function.path.path.starts_with(&base.0) {
                function.path.path.add_prelude(&base.0);
            }
            self.functions.insert(id, function);
        }
    }

    pub fn extract_var_paths(&mut self, context: &Context, base: &VarPath, array: &ShapeRef) {
        for (path, (id, comptime)) in &context.var_paths {
            if path.starts_with(&base.0) {
                let mut path = path.clone();
                path.remove_prelude(&base.0);
                if !path.0.is_empty() {
                    let mut comptime = comptime.clone();
                    for _ in 0..array.dims() {
                        comptime.r#type.array.remove(0);
                    }
                    self.var_paths.insert(path, (*id, comptime));
                }
            }
        }
    }

    pub fn extract_interface_member(
        &mut self,
        base: StrId,
        array: &ShapeRef,
        component: Interface,
        modport: Option<StrId>,
        clock_domain: ClockDomain,
        token: TokenRange,
    ) {
        let mut inserted = HashSet::default();

        let modport_members = if let Some(x) = &modport {
            component.get_modport(x)
        } else {
            HashMap::default()
        };

        let mut id_map = HashMap::default();
        for mut variable in component.variables.into_values() {
            if modport.is_some() {
                if let Some(x) = modport_members.get(&variable.path.first()) {
                    variable.kind = match x {
                        Direction::Input => VarKind::Input,
                        Direction::Output => VarKind::Output,
                        Direction::Inout => VarKind::Inout,
                        _ => variable.kind,
                    };
                } else {
                    // Skip non-modport member
                    continue;
                }
            }

            let path_offset = variable.path.0.len();
            variable.prepend_array_at_path(array, path_offset);

            // override token, affiliation to interface instance
            variable.token = token;
            variable.affiliation = self.get_affiliation();

            inserted.insert(variable.path.clone());
            let comptime = Comptime::from_type(variable.r#type.clone(), clock_domain, token);
            variable.path.add_prelude(&[base]);
            let id = self.insert_var_path(variable.path.clone(), comptime);

            // id mapping for struct/union members
            id_map.insert(variable.id, id);

            variable.id = id;
            self.insert_variable(id, variable);
        }

        // import non-variable VarPath
        for (mut path, (id, mut comptime)) in component.var_paths {
            if !inserted.contains(&path) {
                path.add_prelude(&[base]);
                comptime.r#type.prepend_array(array);

                if let Some(id) = id_map.get(&id) {
                    self.insert_var_path_with_id(path, *id, comptime);
                } else {
                    self.insert_var_path(path, comptime);
                }
            }
        }
    }

    pub fn inc_select_dim(&mut self) {
        if let Some(x) = self.select_dims.last_mut() {
            *x += 1;
        }
    }

    /// A select dimension is relative to its path segment, so a member access
    /// starts the count again.
    pub fn reset_select_dim(&mut self) {
        if let Some(x) = self.select_dims.last_mut() {
            *x = 0;
        }
    }

    pub fn get_select_dim(&self) -> Option<usize> {
        self.select_dims.last().copied()
    }

    pub fn set_default_clock(&mut self, path: VarPath, id: SymbolId) {
        self.default_clock.replace((path, id));
    }

    pub fn set_default_reset(&mut self, path: VarPath, id: SymbolId) {
        self.default_reset.replace((path, id));
    }

    pub fn get_default_clock(&self) -> Option<(FfClock, SymbolId)> {
        if let Some(x) = &self.default_clock {
            if let Some((id, comptime)) = &self.var_paths.get(&x.0) {
                let ret = FfClock {
                    id: *id,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    comptime: comptime.clone(),
                };
                Some((ret, x.1))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_default_reset(&self) -> Option<(FfReset, SymbolId)> {
        if let Some(x) = &self.default_reset {
            if let Some((id, comptime)) = &self.var_paths.get(&x.0) {
                let ret = FfReset {
                    id: *id,
                    index: VarIndex::default(),
                    select: VarSelect::default(),
                    comptime: comptime.clone(),
                };
                Some((ret, x.1))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn insert_inst_signature(&mut self, identifier: &Identifier, sig: &Signature) {
        self.inst_signatures.insert(identifier.text(), sig.clone());
    }

    pub fn collect_modport_signatures(
        &mut self,
        inst: &ComponentInstantiation,
    ) -> HashMap<StrId, Signature> {
        let mut signatures = HashMap::default();
        if !self.inst_signatures.is_empty()
            && let Some(ref opt2) = inst.component_instantiation_opt2
            && let Some(ref port_opt) = opt2.inst_port.inst_port_opt
        {
            let port_items: Vec<_> = port_opt.inst_port_list.as_ref().into();
            for port_item in &port_items {
                let inst_name = if let Some(ref x) = port_item.inst_port_item_opt {
                    let Some(identifier) = x.expression.unwrap_identifier().map(|x| x.identifier())
                    else {
                        continue;
                    };
                    identifier.token.text
                } else {
                    port_item.identifier.text()
                };
                if let Some(sig) = self.inst_signatures.get(&inst_name) {
                    let port_name = port_item.identifier.text();
                    signatures.insert(port_name, sig.clone());
                }
            }
        }

        self.modport_signatures.push(signatures.clone());
        signatures
    }

    pub fn pop_modport_signatures(&mut self) {
        self.modport_signatures.pop();
    }

    pub fn get_modport_signature(&self, port_identifier: &Identifier) -> Option<Signature> {
        if let Some(signatures) = self.modport_signatures.last() {
            signatures.get(&port_identifier.text()).cloned()
        } else {
            None
        }
    }

    pub fn check_size(&mut self, x: usize, token: TokenRange) -> Option<usize> {
        if x > self.config.evaluate_size_limit {
            self.insert_error(AnalyzerError::exceed_limit(
                ExceedLimitKind::EvaluateSize,
                x,
                &token,
            ));
            None
        } else {
            Some(x)
        }
    }

    pub fn block<F, T>(&mut self, f: F) -> IrResult<T>
    where
        F: FnOnce(&mut Context) -> IrResult<T>,
    {
        f(self)
    }

    pub fn push_instance_history(
        &mut self,
        x: Signature,
        site: Option<TokenRange>,
    ) -> Result<Reach, InstanceHistoryError> {
        self.instance_history.push(x, site, &self.config)
    }

    pub fn pop_instance_history(&mut self) {
        self.instance_history.pop();
    }

    pub fn get_current_site(&self) -> Option<TokenRange> {
        self.instance_history.get_current_site()
    }

    pub fn get_current_signature(&self) -> Option<&Signature> {
        self.instance_history.get_current_signature()
    }

    pub fn get_instance_history(&self, sig: &Signature) -> Option<(Arc<Component>, bool)> {
        self.instance_history.get(sig)
    }

    pub fn set_instance_history(&mut self, sig: &Signature, component: Arc<Component>) {
        self.instance_history.set(sig, component, self.in_generic);
    }

    pub fn remove_instance_history(&mut self, sig: &Signature) {
        self.instance_history.remove(sig);
    }

    pub fn push_hierarchy(&mut self, x: StrId) {
        self.hierarchy.push(x);
        self.hierarchical_variables.push(vec![]);
        self.hierarchical_functions.push(vec![]);
    }

    /// The current generate-block prefix (e.g. `["g_leaf[0]"]`), the same prefix
    /// applied to variable paths and to `InstDeclaration::hierarchy`.
    pub fn current_hierarchy(&self) -> &[StrId] {
        &self.hierarchy
    }

    pub fn pop_hierarchy(&mut self) {
        self.hierarchy.pop();
        let drops = self.hierarchical_variables.pop();
        if let Some(drops) = drops {
            for x in drops {
                if let Some(y) = self.shadowed_variables.get_mut(&x)
                    && let Some(y) = y.pop()
                {
                    self.var_paths.insert(x, y);
                } else {
                    self.var_paths.remove(&x);
                }
            }
        }
        let drops = self.hierarchical_functions.pop();
        if let Some(drops) = drops {
            for x in drops {
                self.func_paths.remove(&x);
            }
        }
    }

    pub fn push_affiliation(&mut self, x: Affiliation) {
        self.affiliation.push(x);
    }

    pub fn pop_affiliation(&mut self) {
        self.affiliation.pop();
    }

    pub fn push_override(
        &mut self,
        namespace: Namespace,
        values: HashMap<VarPath, (Comptime, Expression)>,
    ) {
        self.overrides.push(ParameterOverride { namespace, values });
    }

    pub fn pop_override(&mut self) {
        self.overrides.pop();
    }

    pub fn take_override(&mut self) -> HashMap<VarPath, (Comptime, Expression)> {
        self.overrides.pop().map(|x| x.values).unwrap_or_default()
    }

    /// The parameters declared after this one are sized under its value.
    pub fn insert_override(&mut self, path: VarPath, value: (Comptime, Expression)) {
        if let Some(x) = self.overrides.last_mut() {
            x.values.insert(path, value);
        }
    }

    pub fn push_generic_map(&mut self, x: Vec<GenericMap>) {
        self.generic_maps.push(x);
    }

    pub fn pop_generic_map(&mut self) {
        self.generic_maps.pop();
    }

    /// Marks `id` as being expanded; returns false if it already is (a cycle).
    pub fn push_typedef_visiting(&mut self, id: SymbolId) -> bool {
        if self.typedef_visiting.contains(&id) {
            false
        } else {
            self.typedef_visiting.push(id);
            true
        }
    }

    pub fn pop_typedef_visiting(&mut self) {
        self.typedef_visiting.pop();
    }

    pub fn is_affiliated(&self, value: Affiliation) -> bool {
        if let Some(x) = self.affiliation.last() {
            *x == value
        } else {
            false
        }
    }

    pub fn get_affiliation(&self) -> Affiliation {
        self.affiliation.last().copied().unwrap()
    }

    pub fn push_namespace(&mut self, namespace: Namespace) {
        self.namespaces.push(namespace);
    }

    pub fn pop_namespace(&mut self) {
        self.namespaces.pop();
    }

    pub fn push_global_func_caller(&mut self, namespace: Namespace) {
        self.global_func_callers.push(namespace);
    }

    pub fn pop_global_func_caller(&mut self) {
        self.global_func_callers.pop();
    }

    /// Whether the namespace is a caller's one.
    pub fn is_global_func_caller(&self, namespace: &Namespace) -> bool {
        self.global_func_callers
            .iter()
            .any(|x| namespace.included(x))
    }

    pub fn enter_component_sizing(&mut self) {
        self.component_sizing += 1;
    }

    pub fn leave_component_sizing(&mut self) {
        self.component_sizing -= 1;
    }

    /// Whether the component's own variables are out of reach on purpose, so
    /// that resolving through the symbol route says nothing about order.
    pub fn sizing_component_width(&self) -> bool {
        self.component_sizing > 0
    }

    pub fn current_namespace(&self) -> Option<Namespace> {
        self.namespaces.last().cloned()
    }

    pub fn find_path(&self, path: &VarPath) -> Option<(VarId, Comptime)> {
        self.var_paths.get(path).cloned()
    }

    pub fn get_variable(&self, id: &VarId) -> Option<Variable> {
        self.variables.get(id).cloned()
    }

    pub fn remove_path(&mut self, path: &VarPath) {
        self.var_paths.remove(path);
    }

    pub fn drain_var_paths(&mut self) -> HashMap<VarPath, (VarId, Comptime)> {
        std::mem::take(&mut self.var_paths)
    }

    pub fn drain_func_paths(&mut self) -> HashMap<FuncPath, VarId> {
        std::mem::take(&mut self.func_paths)
    }

    pub fn drain_variables(&mut self) -> HashMap<VarId, Variable> {
        std::mem::take(&mut self.variables)
    }

    pub fn drain_port_types(&mut self) -> HashMap<VarPath, (Type, ClockDomain)> {
        std::mem::take(&mut self.port_types)
    }

    pub fn drain_functions(&mut self) -> HashMap<VarId, Function> {
        std::mem::take(&mut self.functions)
    }

    pub fn drain_modports(&mut self) -> HashMap<StrId, Vec<(StrId, Direction)>> {
        std::mem::take(&mut self.modports)
    }

    pub fn drain_declarations(&mut self) -> Vec<Declaration> {
        std::mem::take(&mut self.declarations)
    }

    pub fn drain_errors(&mut self) -> Vec<AnalyzerError> {
        std::mem::take(&mut self.errors.errors)
    }
}

#[cfg(test)]
mod analysis_transaction_tests {
    use super::*;
    use crate::value::Value;

    fn test_variable(id: VarId, value: u64) -> Variable {
        Variable::new(
            id,
            VarPath::default(),
            VarKind::Variable,
            Type::default(),
            vec![Value::new(value, 1, false)],
            Affiliation::Module,
            &TokenRange::default(),
            1,
        )
    }

    fn value(context: &Context, id: VarId) -> &Value {
        &context.variables[&id].value[0]
    }

    #[test]
    fn nested_analysis_transactions_restore_each_snapshot() {
        let id = VarId::from_raw(1);
        let mut context = Context::default();
        context.variables.insert(id, test_variable(id, 0));

        context.begin_analysis_transaction();
        context.variable_mut(&id).unwrap().value[0] = Value::new(1, 1, false);
        context.begin_analysis_transaction();
        context.variable_mut(&id).unwrap().value[0] = Value::new(0, 1, false);

        context.rollback_analysis_transaction();
        assert_eq!(value(&context, id), &Value::new(1, 1, false));
        context.rollback_analysis_transaction();
        assert_eq!(value(&context, id), &Value::new(0, 1, false));
    }

    #[test]
    fn reused_analysis_transactions_do_not_leak_variable_state() {
        let id = VarId::from_raw(1);
        let mut context = Context::default();
        context.variables.insert(id, test_variable(id, 0));

        for _ in 0..2 {
            context.begin_analysis_transaction();
            assert_eq!(value(&context, id), &Value::new(0, 1, false));
            context.variable_mut(&id).unwrap().value[0] = Value::new(1, 1, false);
            context.rollback_analysis_transaction();
        }
        assert_eq!(value(&context, id), &Value::new(0, 1, false));
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static PACKAGE_CONST_EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
