use crate::conv::Context;
use crate::ir::assign_table::AssignTable;
use crate::ir::bigint::gen_mask_range;
use crate::ir::{AssignDestination, Expression, Statement, Type, TypedValue, VarId, VarPath};
use crate::symbol::{Direction, Port, Symbol, SymbolKind};
use crate::symbol_table;
use crate::{AnalyzerError, HashMap};
use indent::indent_all_by;
use std::fmt;
use veryl_parser::resource_table::StrId;
use veryl_parser::token_range::TokenRange;

#[derive(Clone)]
pub struct Function {
    pub id: VarId,
    pub path: VarPath,
    pub r#type: Option<Type>,
    pub functions: Vec<FunctionBody>,
}

impl Function {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.functions {
            x.eval_assign(context, assign_table);
        }
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        for x in &mut self.functions {
            x.rename(table);
        }
    }
}

#[derive(Clone)]
pub struct FunctionBody {
    pub ret: Option<VarId>,
    pub ports: HashMap<StrId, VarId>,
    pub statements: Vec<Statement>,
}

impl FunctionBody {
    pub fn eval_assign(&self, context: &mut Context, assign_table: &mut AssignTable) {
        for x in &self.statements {
            x.eval_assign(context, assign_table, true);
        }
    }

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        for x in &mut self.statements {
            x.rename(table);
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ret = String::new();

        for (i, f) in self.functions.iter().enumerate() {
            if self.functions.len() == 1 {
                ret.push_str(&format!("func {}({}) {{\n", self.id, self.path));
            } else {
                ret.push_str(&format!("func {}[{}]({}) {{\n", self.id, i, self.path));
            }

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
    pub ret: Option<TypedValue>,
    pub inputs: HashMap<StrId, Expression>,
    pub outputs: HashMap<StrId, Vec<AssignDestination>>,
    pub token: TokenRange,
}

impl FunctionCall {
    pub fn eval_assign(
        &self,
        context: &mut Context,
        assign_table: &mut AssignTable,
        in_comb: bool,
    ) {
        for output in self.outputs.values() {
            for dst in output {
                if let Some(index) = dst.index.eval(&context.variables) {
                    let variable = context.variables.get(&dst.id).unwrap();
                    if let Some((beg, end)) =
                        dst.select.eval(&context.variables, &variable.r#type.width)
                    {
                        let mask = gen_mask_range(beg, end);
                        let (success, tokens) =
                            assign_table.insert(variable, index, mask, self.token);
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

    pub fn rename(&mut self, table: &HashMap<VarId, VarId>) {
        if let Some(x) = table.get(&self.id) {
            self.id = *x;
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
        format!("{}({})", self.id, args).fmt(f)
    }
}

pub enum Arguments {
    Positional(Vec<(Expression, Vec<AssignDestination>)>),
    Named(Vec<(StrId, (Expression, Vec<AssignDestination>))>),
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
    ) -> Vec<Expression> {
        let ret = match self {
            Arguments::Positional(x) => x.into_iter().map(|x| x.0).collect(),
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
    ) -> (
        HashMap<StrId, Expression>,
        HashMap<StrId, Vec<AssignDestination>>,
    ) {
        let mut inputs = HashMap::default();
        let mut outputs = HashMap::default();

        let ports = get_ports(&symbol.kind);
        let arity = ports.len();

        if arity != self.len() {
            context.insert_error(AnalyzerError::mismatch_function_arity(
                &symbol.token.text.to_string(),
                arity,
                self.len(),
                &symbol.token.into(),
            ));
            return (inputs, outputs);
        }

        let mut port_map = HashMap::default();
        for port in &ports {
            let name = port.name();
            port_map.insert(name, port.clone());
        }

        match self {
            Arguments::Positional(x) => {
                for (i, (expr, dst)) in x.into_iter().enumerate() {
                    let port = &ports[i];
                    let name = port.name();
                    let direction = port.property().direction;

                    match direction {
                        Direction::Input => {
                            inputs.insert(name, expr);
                        }
                        Direction::Output => {
                            outputs.insert(name, dst);
                        }
                        _ => (),
                    }
                }
            }
            Arguments::Named(x) => {
                for (name, (expr, dst)) in x {
                    if let Some(port) = port_map.get(&name) {
                        let direction = port.property().direction;

                        match direction {
                            Direction::Input => {
                                inputs.insert(name, expr);
                            }
                            Direction::Output => {
                                outputs.insert(name, dst);
                            }
                            _ => (),
                        }
                    }
                }
            }
            Arguments::Null => (),
        };
        (inputs, outputs)
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
