use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::utils::calc_index;
use crate::ir::{
    AssignDestination, Comptime, Expression, IrResult, Signature, Statement, Type, TypeKind,
    ValueVariant, VarId, VarIndex, VarPath, VarPathSelect, VarSelect,
};
use crate::symbol::{Direction, Port, Symbol, SymbolId, SymbolKind};
use crate::symbol_table;
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
pub struct Function {
    pub id: VarId,
    pub path: FuncPath,
    pub r#type: Option<Type>,
    pub array: Vec<usize>,
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
        let index = calc_index(index, &self.array)?;
        self.functions.get(index).cloned()
    }
}

#[derive(Clone)]
pub struct FunctionBody {
    pub ret: Option<VarId>,
    pub ports: HashMap<VarPath, VarId>,
    pub statements: Vec<Statement>,
}

impl FunctionBody {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, true);
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
            let id = func.ports.get(path)?;
            let value = expr.eval_value(context, None)?;
            let var = context.variables.get_mut(id)?;
            var.set_value(&[], value);
        }

        for x in &func.statements {
            x.eval_value(context);
        }

        // TODO get outputs

        if let Some(x) = &func.ret {
            let variable = context.variables.get(x)?;
            variable.get_value(&[])
        } else {
            None
        }
    }

    pub fn eval_comptime(&self, context: &mut Context) -> Comptime {
        let value = self.eval_value(context);
        let value = if let Some(x) = value {
            ValueVariant::Numeric(x)
        } else {
            ValueVariant::Unknown
        };

        if let Some(x) = &self.ret {
            let mut ret = x.clone();
            ret.value = value;
            ret
        } else {
            Comptime::create_unknown(self.token)
        }
    }

    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        for output in self.outputs.values() {
            for dst in output {
                if let Some(index) = dst.index.eval_value(context) {
                    let variable = context.variables.get(&dst.id).cloned().unwrap();
                    if let Some((beg, end)) = dst.select.eval_value(context, &variable.r#type.width)
                    {
                        let mask = gen_mask_range(beg, end);
                        let (success, tokens) =
                            assign_table.insert(&variable, index, mask, self.token);
                        if !success & !in_comb {
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

pub type PositionalArgs = Vec<(Expression, Vec<AssignDestination>, TokenRange)>;
pub type NamedArgs = Vec<(StrId, (Expression, Vec<AssignDestination>, TokenRange))>;
pub type FunctionArgs = (
    HashMap<VarPath, Expression>,
    HashMap<VarPath, Vec<AssignDestination>>,
);

pub enum Arguments {
    Positional(PositionalArgs),
    Named(NamedArgs),
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
            Arguments::Null => 0,
        }
    }

    pub fn to_system_function_args(
        self,
        _context: &mut Context,
        symbol: &Symbol,
    ) -> Vec<(Expression, Vec<AssignDestination>, TokenRange)> {
        let ret = match self {
            Arguments::Positional(x) => x,
            Arguments::Named(_) => {
                // TODO error
                return vec![];
            }
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
        symbol: &Symbol,
    ) -> IrResult<FunctionArgs> {
        let mut inputs = HashMap::default();
        let mut outputs = HashMap::default();

        let ports = get_ports(&symbol.kind);
        let arity = ports.len();
        let token: TokenRange = symbol.token.into();

        if arity != self.len() {
            context.insert_error(AnalyzerError::mismatch_function_arity(
                &symbol.token.text.to_string(),
                arity,
                self.len(),
                &symbol.token.into(),
            ));
            return Err(ir_error!(token));
        }

        let mut port_map = HashMap::default();
        for port in &ports {
            let name = port.name();
            port_map.insert(name, port.clone());
        }

        let mut connections = vec![];
        match self {
            Arguments::Positional(x) => {
                for (i, (expr, dst, _)) in x.into_iter().enumerate() {
                    let port = &ports[i];
                    let name = port.name();
                    let path = VarPath::new(name);
                    let property = port.property();
                    let direction = property.direction;
                    let r#type = property.r#type.to_ir_type(context)?;

                    connections.push((path, r#type, direction, expr, dst));
                }
            }
            Arguments::Named(x) => {
                for (name, (expr, dst, _)) in x {
                    if let Some(port) = port_map.get(&name) {
                        let path = VarPath::new(name);
                        let property = port.property();
                        let direction = property.direction;
                        let r#type = property.r#type.to_ir_type(context)?;

                        connections.push((path, r#type, direction, expr, dst));
                    }
                }
            }
            Arguments::Null => (),
        };

        for (path, r#type, direction, expr, dst) in connections {
            // TODO type compatibility check
            match direction {
                Direction::Input => {
                    inputs.insert(path, expr);
                }
                Direction::Output => {
                    outputs.insert(path, dst);
                }
                Direction::Modport => {
                    if dst.len() != 1 {
                        // TODO modport concatenation error
                        return Err(ir_error!(token));
                    }

                    let arg_path = &dst[0].path;
                    let arg_token = dst[0].token;
                    let mut arg_comptime = expr.eval_comptime(context, None);
                    let mut arg_members = arg_comptime.r#type.modport_members(arg_path);

                    // TODO disable generic_parameters temporarily until r#type can trace generic_parameters
                    if let TypeKind::Modport(x, _) = &mut arg_comptime.r#type.kind {
                        x.generic_parameters.clear();
                    }

                    if !arg_comptime.r#type.compatible(&r#type) {
                        // TODO incompatible modport error
                        return Err(ir_error!(token));
                    }

                    let members = r#type.modport_members(&path);

                    for (name, (path, direction)) in members {
                        if let Some((arg_path, _)) = arg_members.remove(&name) {
                            match direction {
                                Direction::Input => {
                                    let expr =
                                        VarPathSelect(arg_path, VarSelect::default(), arg_token);
                                    let expr = expr.to_expression(context);
                                    if let Some(expr) = expr {
                                        inputs.insert(path, expr);
                                    }
                                }
                                Direction::Output => {
                                    let dst =
                                        VarPathSelect(arg_path, VarSelect::default(), arg_token);
                                    let dst = dst.to_assign_destination(context);
                                    if let Some(dst) = dst {
                                        outputs.insert(path, vec![dst]);
                                    }
                                }
                                _ => (),
                            }
                        }
                    }
                }
                _ => (),
            }
        }

        Ok((inputs, outputs))
    }
}

fn get_ports(kind: &SymbolKind) -> Vec<Port> {
    match kind {
        SymbolKind::GenericInstance(x) => {
            let base = symbol_table::get(x.base).unwrap();
            get_ports(&base.kind)
        }
        SymbolKind::Function(x) => x.ports.clone(),
        SymbolKind::ModportFunctionMember(x) => {
            if let SymbolKind::Function(x) = symbol_table::get(x.function).unwrap().kind {
                x.ports.clone()
            } else {
                unreachable!();
            }
        }
        _ => vec![],
    }
}
