use crate::conv::Context;
use crate::conv::utils::{
    check_compatibility, check_implicit_clock_conversion, eval_array_literal,
};
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::ff_table::AssignTarget;
use crate::ir::{
    AssignDestination, Comptime, Expression, FfTable, IrResult, Shape, ShapeRef, Signature,
    Statement, ValueVariant, VarId, VarIndex, VarPath, VarPathSelect, VarSelect,
};
use crate::symbol::{Direction, Symbol, SymbolId, SymbolKind};
use crate::value::{Value, ValueBigUint};
use crate::{AnalyzerError, HashMap, HashSet, ir_error};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct FuncPath {
    pub path: VarPath,
    pub sig: Signature,
}

impl FuncPath {
    pub fn new(id: SymbolId) -> Self {
        Self {
            path: VarPath::default(),
            sig: Signature::new(id),
        }
    }

    pub fn add_prelude(&mut self, x: &[StrId]) {
        self.path.add_prelude(x)
    }

    pub fn base(&self) -> FuncPath {
        let mut ret = self.clone();
        ret.sig.parameters.clear();
        ret.sig.generic_parameters.clear();
        ret
    }
}

impl fmt::Display for FuncPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        if !self.path.0.is_empty() {
            ret.push_str(&format!("{}.", self.path));
        }
        ret.push_str(&format!("{}", self.sig));

        ret.fmt(f)
    }
}

#[derive(Clone)]
pub struct FuncArg {
    pub name: StrId,
    pub comptime: Comptime,
    pub members: Vec<(VarPath, Comptime, Direction)>,
}

#[derive(Clone)]
pub struct Function {
    pub name: StrId,
    pub id: VarId,
    pub path: FuncPath,
    pub r#type: Comptime,
    pub array: Shape,
    pub arity: usize,
    pub args: Vec<FuncArg>,
    pub receiver_relative: bool,
    /// Storage objects owned by this receiver, independent of whether the
    /// receiver itself is scalar or arrayed. This ownership is what decides
    /// whether an enclosing receiver axis composes with a nested method.
    pub receiver_variables: HashSet<VarId>,
    /// Number of leading receiver coordinates owned by each referenced
    /// storage object. Different variables can belong to different nested
    /// receiver axes (for example outer interface and modport-array formal).
    pub receiver_prefixes: HashMap<VarId, usize>,
    pub is_const: bool,
    pub functions: Vec<FunctionBody>,
    pub token: TokenRange,
}

impl Function {
    fn contains_receiver_index(&self, index: &[usize]) -> bool {
        if self.array.is_empty() {
            return index.is_empty();
        }
        if self.array.dims() == 1 && self.array[0] == Some(1) && index.is_empty() {
            return true;
        }
        index.len() == self.array.dims()
            && index
                .iter()
                .zip(self.array.iter())
                .all(|(index, length)| length.is_some_and(|length| *index < length))
    }

    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        let receiver_count = self.array.total().unwrap_or(0);
        if !self.array.is_empty() && receiver_count <= assign_table.array_limit {
            for flat in 0..receiver_count {
                let index = VarIndex::from_index(flat, &self.array);
                let Some(index) = index.eval_value(context) else {
                    continue;
                };
                if let Some(body) = self.get_function(&index) {
                    body.eval_assign(context, assign_table);
                }
            }
        } else {
            for body in &self.functions {
                body.eval_assign(context, assign_table);
            }
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for body in &mut self.functions {
            body.set_index(index);
        }
    }

    pub(crate) fn prepend_receiver(
        &mut self,
        array: &ShapeRef,
        receiver_variables: &HashSet<VarId>,
        receiver_functions: &HashSet<VarId>,
    ) {
        self.receiver_variables
            .extend(receiver_variables.iter().copied());
        if array.is_empty() {
            return;
        }
        let mut combined = array.to_owned();
        combined.append(&mut self.array);
        self.array = combined;
        let added_dims = array.dims();
        for variable in receiver_variables {
            self.receiver_prefixes
                .entry(*variable)
                .and_modify(|prefix| *prefix += added_dims)
                .or_insert(added_dims);
        }
        for body in &mut self.functions {
            body.prepend_call_receiver(added_dims, receiver_functions);
        }
    }

    pub fn get_function(&self, index: &[usize]) -> Option<FunctionBody> {
        if self.array.is_empty() {
            self.contains_receiver_index(index).then_some(())?;
            return self.functions.first().cloned();
        }

        self.contains_receiver_index(index).then_some(())?;
        let flat = self.array.calc_index(index)?;
        let mut body = self.functions.first()?.clone();
        let receiver_index = VarIndex::from_index(flat, &self.array);
        body.set_index_with_receiver(&receiver_index, &self.receiver_prefixes);
        Some(body)
    }

    pub(crate) fn get_function_for_index(&self, index: &VarIndex) -> Option<FunctionBody> {
        if self.array.is_empty() {
            if !index.0.is_empty() {
                return None;
            }
            return self.functions.first().cloned();
        }
        let concrete = index
            .0
            .iter()
            .map(|expression| expression.comptime().get_value().ok()?.to_usize())
            .collect::<Option<Vec<_>>>();
        if concrete
            .as_deref()
            .is_some_and(|index| !self.contains_receiver_index(index))
        {
            return None;
        }
        self.array.calc_index_expr(&index.0)?;
        let mut body = self.functions.first()?.clone();
        body.set_index_with_receiver(index, &self.receiver_prefixes);
        Some(body)
    }

    pub fn to_proto(&self) -> FuncProto {
        FuncProto {
            name: self.name,
            id: self.id,
            r#type: self.r#type.clone(),
            arity: self.arity,
            args: self.args.clone(),
            receiver_relative: self.receiver_relative,
            token: self.token,
        }
    }
}

#[derive(Clone)]
pub struct FuncProto {
    pub name: StrId,
    pub id: VarId,
    pub r#type: Comptime,
    pub arity: usize,
    pub args: Vec<FuncArg>,
    pub receiver_relative: bool,
    pub token: TokenRange,
}

#[derive(Clone)]
pub struct FunctionBody {
    pub ret: Option<VarId>,
    pub arg_map: HashMap<VarPath, VarId>,
    pub statements: Vec<Statement>,
}

impl FunctionBody {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, AssignContext::Function, &[]);
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for statement in &mut self.statements {
            statement.set_index(index);
        }
    }

    pub(crate) fn set_index_with_receiver(
        &mut self,
        index: &VarIndex,
        receiver_prefixes: &HashMap<VarId, usize>,
    ) {
        for x in &mut self.statements {
            x.set_index_with_receiver(index, receiver_prefixes);
        }
    }

    fn prepend_call_receiver(&mut self, dims: usize, receiver_functions: &HashSet<VarId>) {
        for statement in &mut self.statements {
            statement.prepend_call_receiver(dims, receiver_functions);
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, f) in self.functions.iter().enumerate() {
            if !self.array.is_empty() {
                ret.push_str(&format!("func {}[*]({})", self.id, self.path));
            } else if self.functions.len() == 1 {
                ret.push_str(&format!("func {}({})", self.id, self.path));
            } else {
                ret.push_str(&format!("func {}[{}]({})", self.id, i, self.path));
            }

            if let Some(x) = f.ret {
                ret.push_str(&format!(" -> {x}"));
            }
            ret.push_str(" {\n");

            for s in &f.statements {
                let text = format!("{}\n", s);
                ret.push_str(&indent_all_by(2, text));
            }

            ret.push_str("}\n");
        }

        ret.trim_end().fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct FunctionCall {
    pub id: VarId,
    /// Receiver coordinates retained as expressions. `index` is the constant
    /// fast path; this form preserves runtime interface-array selection.
    pub receiver_index: VarIndex,
    /// Leading coordinates supplied by the enclosing receiver. This is tied
    /// to the target function id when receiver axes are composed.
    pub receiver_prefix_dims: usize,
    pub index: Option<Vec<usize>>,
    pub comptime: Comptime,
    pub inputs: CallArgs<Expression>,
    pub outputs: CallArgs<Vec<AssignDestination>>,
    pub receiver_relative: bool,
}

impl FunctionCall {
    pub fn eval_type(&mut self, context: &mut Context) {
        self.comptime.is_const = self.eval_comptime_flag(context);
    }

    pub fn eval_value(&self, context: &mut Context) -> Option<Value> {
        let func = context.functions.get(&self.id)?;
        let func = func.get_function_for_index(&self.receiver_index)?;

        // set inputs
        for (path, expr) in &self.inputs {
            let id = func.arg_map.get(path)?;
            let value = expr.eval_value(context)?;
            let var = context.variable_mut(id)?;
            var.set_value(&[], value, None);
        }

        let disable_const_opt = context.disalbe_const_opt;
        context.disalbe_const_opt = true;
        let prior_overflow = context.comptime_for_overflow.take();
        for x in &func.statements {
            x.eval_value(context);
        }
        let overflowed = context.comptime_for_overflow.is_some();
        if let Some(x) = prior_overflow {
            context.comptime_for_overflow.get_or_insert(x);
        }
        context.disalbe_const_opt = disable_const_opt;

        // TODO get outputs

        // A skipped over-limit for loop leaves the return value at its
        // pre-loop state; treat the call as unevaluable instead.
        if overflowed {
            return None;
        }

        if let Some(x) = &func.ret {
            let variable = context.variables.get(x)?;
            variable.get_value(&[]).cloned()
        } else {
            None
        }
    }

    pub fn eval_comptime(&mut self, context: &mut Context) -> Comptime {
        let value = self.eval_value(context);
        let value = if let Some(x) = value {
            ValueVariant::Numeric(x)
        } else {
            ValueVariant::Unknown
        };

        let mut ret = self.comptime.clone();
        ret.value = value;

        ret.is_const = self.eval_comptime_flag(context);
        ret
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        // Record reads from input expressions so that downstream consumers
        // of `AssignTable.refernced` (e.g. the combinational-loop detector)
        // see input-variable references at the call site.
        for expr in self.inputs.values() {
            expr.eval_assign(context, assign_table, assign_context);
        }
        for expression in &self.receiver_index.0 {
            expression.eval_assign(context, assign_table, assign_context);
        }
        for output in self.outputs.values() {
            for dst in output {
                if let Some(index) = dst.index.eval_value(context) {
                    let variable = context.get_variable_info(dst.id).unwrap();
                    if let Some((beg, end)) = dst.select.conservative_packed_range(
                        context,
                        &variable.r#type,
                        dst.comptime.member_select_domain,
                    ) {
                        let mask = ValueBigUint::gen_mask_range(beg, end);
                        let (success, tokens) = assign_table.insert_assign(
                            &variable,
                            index,
                            mask,
                            false,
                            false,
                            self.comptime.token,
                        );
                        if !success & assign_context.is_ff() {
                            context.insert_error(AnalyzerError::multiple_assignment(
                                &variable.path.to_string(),
                                &self.comptime.token,
                                &tokens,
                            ));
                        }
                    }
                }
            }
        }
    }

    pub fn gather_ff(
        &self,
        context: &mut Context,
        table: &mut FfTable,
        decl: usize,
        assign_target: Option<&AssignTarget>,
        from_ff: bool,
    ) {
        for input in self.inputs.values() {
            input.gather_ff(context, table, decl, assign_target, from_ff);
        }
        for expression in &self.receiver_index.0 {
            expression.gather_ff(context, table, decl, assign_target, from_ff);
        }
        for dsts in self.outputs.values() {
            for dst in dsts {
                dst.gather_ff(context, table, decl);
            }
        }
    }

    pub fn gather_ff_comb_assign(&self, context: &mut Context, table: &mut FfTable, decl: usize) {
        for dsts in self.outputs.values() {
            for dst in dsts {
                dst.gather_ff_comb_assign(context, table, decl);
            }
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for input in self.inputs.values_mut() {
            input.set_index(index);
        }
        for outputs in self.outputs.values_mut() {
            for output in outputs {
                output.set_index(index);
            }
        }
    }

    pub(crate) fn set_index_with_receiver(
        &mut self,
        index: &VarIndex,
        receiver_prefixes: &HashMap<VarId, usize>,
    ) {
        // A nested receiver selector may itself reference storage owned by the
        // enclosing receiver. Specialize those references before prepending
        // the enclosing coordinates to the nested receiver path.
        for expression in &mut self.receiver_index.0 {
            expression.set_index_with_receiver(index, receiver_prefixes);
        }
        let mut receiver_valid = true;
        if self.receiver_relative && self.receiver_prefix_dims != 0 {
            if let Some(prefix) = index.0.get(..self.receiver_prefix_dims) {
                self.receiver_index.add_prelude(&VarIndex(prefix.to_vec()));
            } else {
                debug_assert!(
                    false,
                    "receiver prefix has more dimensions than the enclosing receiver index"
                );
                receiver_valid = false;
            }
        }
        self.index = (receiver_valid && self.receiver_index.is_const())
            .then(|| {
                self.receiver_index
                    .0
                    .iter()
                    .map(|expression| expression.comptime().get_value().ok()?.to_usize())
                    .collect()
            })
            .flatten();
        for x in self.inputs.values_mut() {
            x.set_index_with_receiver(index, receiver_prefixes);
        }
        for x in self.outputs.values_mut() {
            for x in x {
                x.set_index_with_receiver(index, receiver_prefixes);
            }
        }
    }

    pub(crate) fn prepend_receiver_axes(&mut self, dims: usize, functions: &HashSet<VarId>) {
        if functions.contains(&self.id) {
            self.receiver_prefix_dims += dims;
        }
        for expression in &mut self.receiver_index.0 {
            expression.prepend_call_receiver(dims, functions);
        }
        for expression in self.inputs.values_mut() {
            expression.prepend_call_receiver(dims, functions);
        }
        for destinations in self.outputs.values_mut() {
            for destination in destinations {
                destination.prepend_call_receiver(dims, functions);
            }
        }
    }

    fn eval_comptime_flag(&mut self, context: &mut Context) -> bool {
        let mut is_const = context
            .functions
            .get(&self.id)
            .map(|func| func.is_const)
            .unwrap_or(true);
        for expression in &mut self.receiver_index.0 {
            is_const &= expression.eval_comptime(context, None).is_const;
        }
        for expr in self.inputs.values_mut() {
            is_const &= expr.eval_comptime(context, None).is_const;
        }

        // function with side-effect through output ports is not const
        if !self.outputs.is_empty() {
            is_const = false;
        }

        is_const
    }
}

impl fmt::Display for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut args = String::new();

        let mut inputs: Vec<_> = self.inputs.iter().map(|(k, v)| (k, v)).collect();
        let mut outputs: Vec<_> = self.outputs.iter().map(|(k, v)| (k, v)).collect();
        inputs.sort_by_key(|x| x.0);
        outputs.sort_by_key(|x| x.0);

        for (id, val) in &inputs {
            args.push_str(&format!("{id}: {val}, "));
        }
        for (id, val) in &outputs {
            if val.len() == 1 {
                args.push_str(&format!("{id}: {}, ", val[0]));
            } else {
                args.push_str(&format!("{id}: {{{}", val[0]));
                for x in &val[1..] {
                    args.push_str(&format!(", {x}"));
                }
                args.push_str("}}, ");
            }
        }
        let args = if args.is_empty() {
            &args
        } else {
            &args[0..args.len() - 2]
        };

        let mut index = String::new();
        for coordinate in &self.receiver_index.0 {
            index.push_str(&format!("[{coordinate}]"));
        }

        format!("{}{}({})", self.id, index, args).fmt(f)
    }
}

pub type PositionalArgs = Vec<(Expression, Vec<VarPathSelect>, TokenRange)>;
pub type NamedArgs = Vec<(StrId, (Expression, Vec<VarPathSelect>, TokenRange))>;
pub type FunctionArgs = (CallArgs<Expression>, CallArgs<Vec<AssignDestination>>);

/// Argument bindings of a [`FunctionCall`], in source argument order.
/// Not a map: `VarPath` keys are interned `StrId`s whose numeric values
/// vary run-to-run (parallel interning), so map iteration would make the
/// consumers' emission order — and the generated code — nondeterministic.
/// Built only by `to_function_args`, which preserves the source order.
#[derive(Clone, Debug, Default)]
pub struct CallArgs<T>(Vec<(VarPath, T)>);

impl<T> CallArgs<T> {
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.iter_mut().map(|(_, v)| v)
    }
}

impl<T> std::ops::Deref for CallArgs<T> {
    type Target = [(VarPath, T)];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for CallArgs<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, T> IntoIterator for &'a CallArgs<T> {
    type Item = &'a (VarPath, T);
    type IntoIter = std::slice::Iter<'a, (VarPath, T)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Clone)]
pub enum Arguments {
    Positional(PositionalArgs),
    Named(NamedArgs),
    Mixed(PositionalArgs, NamedArgs),
    Null,
}

impl Arguments {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match self {
            Arguments::Positional(x) => x.len(),
            Arguments::Named(x) => x.len(),
            Arguments::Mixed(x, y) => x.len() + y.len(),
            Arguments::Null => 0,
        }
    }

    pub fn to_system_function_args(
        self,
        _context: &mut Context,
        symbol: &Symbol,
    ) -> Vec<(Expression, Vec<VarPathSelect>, TokenRange)> {
        let ret = match self {
            Arguments::Positional(x) => x,
            Arguments::Named(_) => {
                // TODO error
                return vec![];
            }
            Arguments::Mixed(_, _) => vec![],
            Arguments::Null => vec![],
        };

        if let SymbolKind::SystemFunction(x) = &symbol.kind {
            let arity = x.ports.len();

            if arity != ret.len() {
                // TODO
                //let name = symbol.token.text.to_string();
                //context.insert_error(AnalyzerError::mismatch_function_arity(
                //    &name,
                //    arity,
                //    ret.len(),
                //    &symbol.token.into(),
                //))
            }
        }

        ret
    }

    pub fn to_function_args(
        self,
        context: &mut Context,
        func: &FuncProto,
        token: TokenRange,
    ) -> IrResult<FunctionArgs> {
        let mut inputs: Vec<(VarPath, Expression)> = Vec::new();
        let mut outputs: Vec<(VarPath, Vec<AssignDestination>)> = Vec::new();

        if func.arity != self.len() {
            context.insert_error(AnalyzerError::mismatch_function_arity(
                &func.name.to_string(),
                func.arity,
                self.len(),
                &token,
            ));
            return Err(ir_error!(func.token));
        }

        let mut arg_map_by_name = HashMap::default();
        let mut arg_map_by_index = HashMap::default();
        for (i, arg) in func.args.iter().enumerate() {
            arg_map_by_name.insert(arg.name, arg.clone());
            arg_map_by_index.insert(i, arg.clone());
        }

        let mut connections = vec![];
        match self {
            Arguments::Positional(x) => {
                for (i, (expr, dst, _)) in x.into_iter().enumerate() {
                    if let Some(arg) = arg_map_by_index.get(&i) {
                        connections.push((arg, expr, dst));
                    }
                }
            }
            Arguments::Named(x) => {
                for (name, (expr, dst, _)) in x {
                    if let Some(arg) = arg_map_by_name.get(&name) {
                        connections.push((arg, expr, dst));
                    }
                }
            }
            Arguments::Mixed(_, _) => (),
            Arguments::Null => (),
        };

        for (arg, mut expr, dst) in connections {
            if arg.members.len() == 1 {
                let (path, _, direction) = &arg.members[0];
                match direction {
                    Direction::Input => {
                        let arg_type = &arg.comptime.r#type;
                        // Argument expressions are initially converted without a
                        // destination type. Apply the resolved formal shape here,
                        // where the actual-to-formal binding is first available.
                        if matches!(expr, Expression::ArrayLiteral(..)) {
                            eval_array_literal(
                                context,
                                Some(&arg_type.array),
                                Some(arg_type.width()),
                                &mut expr,
                            )?;
                        }
                        if arg_type.is_clock() || arg_type.is_reset() {
                            let expr_comptime = expr.eval_comptime(context, None);
                            let expr_token = expr_comptime.token;
                            check_implicit_clock_conversion(
                                context,
                                arg_type,
                                expr_comptime,
                                &expr_token,
                            );
                        }
                        inputs.push((path.clone(), expr));
                    }
                    Direction::Output => {
                        let dst = dst
                            .into_iter()
                            .filter_map(|x| x.to_assign_destination(context, false))
                            .collect();
                        outputs.push((path.clone(), dst));
                    }
                    _ => (),
                }
            } else {
                let expr_comptime = expr.eval_comptime(context, None);
                let expr_token = expr_comptime.token;
                let expr_members = expr_comptime
                    .r#type
                    .expand_interface(context, &dst[0].0, expr_token)?;

                check_compatibility(context, &arg.comptime.r#type, expr_comptime, &expr_token);

                for (x, y) in arg.members.iter().zip(expr_members.iter()) {
                    let arg_path = x.0.clone();
                    let direction = x.2;
                    let expr_path = y.0.clone();

                    match direction {
                        Direction::Input => {
                            let expr = VarPathSelect(expr_path, VarSelect::default(), expr_token);
                            let expr = expr.to_expression(context);
                            if let Some(expr) = expr {
                                inputs.push((arg_path, expr));
                            }
                        }
                        Direction::Output => {
                            let dst = VarPathSelect(expr_path, VarSelect::default(), expr_token);
                            let dst = dst.to_assign_destination(context, false);
                            if let Some(dst) = dst {
                                outputs.push((arg_path, vec![dst]));
                            }
                        }
                        _ => (),
                    }
                }
            }
        }

        Ok((CallArgs(inputs), CallArgs(outputs)))
    }
}
