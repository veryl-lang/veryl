use crate::conv::Context;
use crate::conv::utils::check_compatibility;
use crate::ir::assign_table::{AssignContext, AssignTable};
use crate::ir::{
    AssignDestination, Comptime, Expression, IrResult, Shape, Signature, Statement, Type,
    ValueVariant, VarId, VarIndex, VarPath, VarPathSelect, VarSelect,
};
use crate::symbol::{ClockDomain, Direction, Symbol, SymbolId, SymbolKind};
use crate::value::{Value, gen_mask_range};
use crate::{AnalyzerError, HashMap, ir_error};
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
pub struct FuncProto {
    pub name: StrId,
    pub id: VarId,
    pub ret: Option<Comptime>,
    pub arity: usize,
    pub args: Vec<FuncArg>,
    pub token: TokenRange,
}

#[derive(Clone)]
pub struct FuncArg {
    pub name: StrId,
    pub comptime: Comptime,
    pub members: Vec<(VarPath, Comptime, Direction)>,
}

#[derive(Clone)]
pub struct Function {
    pub id: VarId,
    pub path: FuncPath,
    pub r#type: Option<Type>,
    pub array: Shape,
    pub functions: Vec<FunctionBody>,
}

impl Function {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.functions {
            x.eval_assign(context, assign_table);
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for x in &mut self.functions {
            x.set_index(index);
        }
    }

    pub fn get_function(&self, index: &[usize]) -> Option<FunctionBody> {
        let index = self.array.calc_index(index)?;
        self.functions.get(index).cloned()
    }
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
        for x in &mut self.statements {
            x.set_index(index);
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, f) in self.functions.iter().enumerate() {
            if self.functions.len() == 1 {
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
    pub index: Option<Vec<usize>>,
    pub ret: Option<Comptime>,
    pub inputs: HashMap<VarPath, Expression>,
    pub outputs: HashMap<VarPath, Vec<AssignDestination>>,
    pub token: TokenRange,
}

impl FunctionCall {
    pub fn eval_value(&self, context: &mut Context) -> Option<Value> {
        let func = context.functions.get(&self.id)?;
        let func = if let Some(x) = &self.index {
            func.get_function(x)
        } else {
            func.get_function(&[])
        }?;

        // set inputs
        for (path, expr) in &self.inputs {
            let id = func.arg_map.get(path)?;
            let value = expr.eval_value(context, None)?;
            let var = context.variables.get_mut(id)?;
            var.set_value(&[], value, None);
        }

        for x in &func.statements {
            x.eval_value(context);
        }

        // TODO get outputs

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

        let mut is_const = true;
        for expr in self.inputs.values_mut() {
            is_const &= expr.eval_comptime(context, None).is_const;
        }

        // function with side-effect through output ports is not const
        if !self.outputs.is_empty() {
            is_const = false;
        }

        let mut ret = if let Some(x) = &self.ret {
            let mut ret = x.clone();
            ret.value = value;
            ret
        } else {
            Comptime::create_unknown(ClockDomain::None, self.token)
        };

        ret.is_const = is_const;
        ret
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        assign_context: AssignContext,
    ) {
        for output in self.outputs.values() {
            for dst in output {
                if let Some(index) = dst.index.eval_value(context) {
                    let variable = context.get_variable_info(dst.id).unwrap();
                    if let Some((beg, end)) =
                        dst.select.eval_value(context, &variable.r#type, false)
                    {
                        let mask = gen_mask_range(beg, end);
                        let (success, tokens) =
                            assign_table.insert_assign(&variable, index, mask, false, self.token);
                        if !success & assign_context.is_ff() {
                            context.insert_error(AnalyzerError::multiple_assignment(
                                &variable.path.to_string(),
                                &self.token,
                                &tokens,
                            ));
                        }
                    }
                }
            }
        }
    }

    pub fn set_index(&mut self, index: &VarIndex) {
        for x in self.inputs.values_mut() {
            x.set_index(index);
        }
        for x in self.outputs.values_mut() {
            for x in x {
                x.set_index(index);
            }
        }
    }
}

impl fmt::Display for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut args = String::new();

        let mut inputs: Vec<_> = self.inputs.iter().collect();
        let mut outputs: Vec<_> = self.outputs.iter().collect();
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
        if let Some(x) = &self.index {
            for x in x {
                index.push_str(&format!("[{}]", x));
            }
        }

        format!("{}{}({})", self.id, index, args).fmt(f)
    }
}

pub type PositionalArgs = Vec<(Expression, Vec<VarPathSelect>, TokenRange)>;
pub type NamedArgs = Vec<(StrId, (Expression, Vec<VarPathSelect>, TokenRange))>;
pub type FunctionArgs = (
    HashMap<VarPath, Expression>,
    HashMap<VarPath, Vec<AssignDestination>>,
);

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
        let mut inputs = HashMap::default();
        let mut outputs = HashMap::default();

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
                        inputs.insert(path.clone(), expr);
                    }
                    Direction::Output => {
                        let dst = dst
                            .into_iter()
                            .filter_map(|x| x.to_assign_destination(context, false))
                            .collect();
                        outputs.insert(path.clone(), dst);
                    }
                    _ => (),
                }
            } else {
                let expr_comptime = expr.eval_comptime(context, None);
                let expr_token = expr_comptime.token;
                let expr_members = expr_comptime
                    .r#type
                    .expand_interface(context, &dst[0].0, expr_token)?;

                check_compatibility(context, &arg.comptime.r#type, &expr_comptime, &expr_token);

                for (x, y) in arg.members.iter().zip(expr_members.iter()) {
                    let arg_path = x.0.clone();
                    let direction = x.2;
                    let expr_path = y.0.clone();

                    match direction {
                        Direction::Input => {
                            let expr = VarPathSelect(expr_path, VarSelect::default(), expr_token);
                            let expr = expr.to_expression(context);
                            if let Some(expr) = expr {
                                inputs.insert(arg_path, expr);
                            }
                        }
                        Direction::Output => {
                            let dst = VarPathSelect(expr_path, VarSelect::default(), expr_token);
                            let dst = dst.to_assign_destination(context, false);
                            if let Some(dst) = dst {
                                outputs.insert(arg_path, vec![dst]);
                            }
                        }
                        _ => (),
                    }
                }
            }
        }

        Ok((inputs, outputs))
    }
}
