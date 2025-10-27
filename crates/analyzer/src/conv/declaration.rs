use crate::analyzer_error::AnalyzerError;
use crate::conv::checker::r#enum::check_enum;
use crate::conv::checker::modport::{check_modport, check_port};
use crate::conv::instance::{InstanceHistoryError, InstanceSignature};
use crate::conv::utils::{eval_expr, eval_range, get_overridden_params};
use crate::conv::{Affiliation, Context, Conv};
use crate::definition_table::Definition;
use crate::ir::{self, TypedValue, Value, VarIndex, VarKind, VarPath, VarPathIndex, Variable};
use crate::symbol::{Direction, SymbolKind};
use crate::{definition_table, symbol_table};
use num_bigint::BigUint;
use veryl_parser::resource_table::{self, StrId};
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_grammar_trait::*;

impl Conv<&GenerateItem> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateItem) -> Self {
        match value {
            GenerateItem::LetDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.let_declaration.as_ref()))
            }
            GenerateItem::VarDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.var_declaration.as_ref()))
            }
            GenerateItem::AlwaysFfDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.always_ff_declaration.as_ref()))
            }
            GenerateItem::AlwaysCombDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.always_comb_declaration.as_ref()))
            }
            GenerateItem::GenerateIfDeclaration(x) => {
                Conv::conv(context, x.generate_if_declaration.as_ref())
            }
            GenerateItem::GenerateForDeclaration(x) => {
                Conv::conv(context, x.generate_for_declaration.as_ref())
            }
            GenerateItem::ConstDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.const_declaration.as_ref()))
            }
            GenerateItem::AssignDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.assign_declaration.as_ref()))
            }
            GenerateItem::FunctionDeclaration(x) => {
                let _: () = Conv::conv(context, x.function_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::EnumDeclaration(x) => {
                let _: () = Conv::conv(context, x.enum_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::InitialDeclaration(x) => {
                let _: () = Conv::conv(context, x.initial_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::FinalDeclaration(x) => {
                let _: () = Conv::conv(context, x.final_declaration.as_ref());
                ir::DeclarationBlock::default()
            }
            GenerateItem::InstDeclaration(x) => {
                ir::DeclarationBlock::new(Conv::conv(context, x.inst_declaration.as_ref()))
            }
            // TODO
            _ => ir::DeclarationBlock::default(),
        }
    }
}

fn get_label(block: &GenerateOptionalNamedBlock, default: StrId) -> StrId {
    if let Some(x) = &block.generate_optional_named_block_opt {
        x.identifier.text()
    } else {
        default
    }
}

impl Conv<&GenerateIfDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateIfDeclaration) -> Self {
        let label = value.generate_named_block.identifier.text();

        let (typed_value, _) = eval_expr(context, None, value.expression.as_ref());

        let Some(cond) = typed_value.get_value() else {
            // TODO evaluation failed
            return ir::DeclarationBlock::default();
        };

        if cond.to_usize() != 0 {
            context.push_hier(label);
            let block: ir::DeclarationBlock =
                Conv::conv(context, value.generate_named_block.as_ref());
            context.pop_hier();
            block
        } else {
            for x in &value.generate_if_declaration_list {
                let (typed_value, _) = eval_expr(context, None, x.expression.as_ref());

                let Some(cond) = typed_value.get_value() else {
                    // TODO evaluation failed
                    return ir::DeclarationBlock::default();
                };

                if cond.to_usize() != 0 {
                    let label = get_label(&x.generate_optional_named_block, label);

                    context.push_hier(label);
                    let block: ir::DeclarationBlock =
                        Conv::conv(context, x.generate_optional_named_block.as_ref());
                    context.pop_hier();
                    return block;
                }
            }

            if let Some(x) = &value.generate_if_declaration_opt {
                let label = get_label(&x.generate_optional_named_block, label);

                context.push_hier(label);
                let block: ir::DeclarationBlock =
                    Conv::conv(context, x.generate_optional_named_block.as_ref());
                context.pop_hier();
                block
            } else {
                ir::DeclarationBlock::default()
            }
        }
    }
}

impl Conv<&GenerateNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateNamedBlock) -> Self {
        let mut ret = vec![];
        for x in &value.generate_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in &items {
                let mut item: ir::DeclarationBlock = Conv::conv(context, item);
                ret.append(&mut item.0);
            }
        }
        ir::DeclarationBlock(ret)
    }
}

impl Conv<&GenerateOptionalNamedBlock> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateOptionalNamedBlock) -> Self {
        let mut ret = vec![];
        for x in &value.generate_optional_named_block_list {
            let items: Vec<_> = x.generate_group.as_ref().into();
            for item in &items {
                let mut item: ir::DeclarationBlock = Conv::conv(context, item);
                ret.append(&mut item.0);
            }
        }
        ir::DeclarationBlock(ret)
    }
}

impl Conv<&GenerateForDeclaration> for ir::DeclarationBlock {
    fn conv(context: &mut Context, value: &GenerateForDeclaration) -> Self {
        let label = value.generate_named_block.identifier.text();
        let range = eval_range(context, &value.range);

        let mut ret = ir::DeclarationBlock::default();

        if let Some((beg, end)) = range {
            for i in beg..end {
                let label = format!("{}[{}]", label, i);
                let label = resource_table::insert_str(&label);

                context.push_hier(label);

                let index = value.identifier.text();
                let path = VarPath::new(index);
                let kind = VarKind::Const;
                let typed_value = TypedValue::create_value(BigUint::from(i), 32);
                let variable = Variable::new(
                    path.clone(),
                    kind,
                    vec![typed_value.get_value().unwrap()],
                    &[],
                );
                context.insert_variable(path, typed_value.clone(), variable);

                let mut block: ir::DeclarationBlock =
                    Conv::conv(context, value.generate_named_block.as_ref());
                ret.0.append(&mut block.0);

                context.pop_hier();
            }
        } else {
            // TODO evaluation failed
        }

        ret
    }
}

impl Conv<&WithParameterItem> for ir::Declaration {
    fn conv(context: &mut Context, value: &WithParameterItem) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();

            let Ok(r#type) = x.r#type.to_ir_type(context) else {
                // TODO type evaluation failed error
                return ir::Declaration::Null;
            };

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return ir::Declaration::Null;
            };

            let (typed_value, _) = eval_expr(context, Some(r#type), expr);

            // Get overridden parameter if it exists
            let Some(value) = context
                .overrides
                .get(&path)
                .cloned()
                .or_else(|| typed_value.get_value())
            else {
                // TODO default value evaluation failed
                return ir::Declaration::Null;
            };

            // TODO for param array
            let variable = Variable::new(path.clone(), kind, vec![value], &[]);
            context.insert_variable(path, typed_value.clone(), variable);
        }
        ir::Declaration::Null
    }
}

impl Conv<&PortDeclarationItem> for ir::Declaration {
    fn conv(context: &mut Context, value: &PortDeclarationItem) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Port(x) = symbol.found.kind
        {
            check_port(context, value);

            let path = VarPath::new(symbol.found.token.text);
            let kind = match x.direction {
                Direction::Input => VarKind::Input,
                Direction::Output => VarKind::Output,
                Direction::Inout => VarKind::Inout,
                _ => {
                    // TODO modport
                    return ir::Declaration::Null;
                }
            };

            let Ok(r#type) = x.r#type.to_ir_type(context) else {
                // TODO type evaluation failed error
                return ir::Declaration::Null;
            };

            if let Some(x) = &x.default_value {
                let (typed_value, _) = eval_expr(context, Some(r#type), x);
                let Some(value) = typed_value.get_value() else {
                    // TODO default value evaluation failed
                    return ir::Declaration::Null;
                };

                // TODO for array
                let variable = Variable::new(path.clone(), kind, vec![value], &[]);
                context.insert_variable(path, typed_value, variable);
            } else {
                let mut values = vec![];
                for _ in 0..r#type.total_array() {
                    values.push(Value::new_x(r#type.total_width(), false));
                }
                let variable = Variable::new(path.clone(), kind, values, &r#type.array);

                let typed_value = TypedValue::from_type(r#type);
                context.insert_variable(path, typed_value, variable);
            }
        }
        ir::Declaration::Null
    }
}

impl Conv<&AlwaysFfDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysFfDeclaration) -> Self {
        // TODO explicit clock / reset
        let Some(clock) = context.get_default_clock() else {
            return ir::Declaration::Null;
        };
        let Some(reset) = context.get_default_reset() else {
            return ir::Declaration::Null;
        };

        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
        let statements = statements.0;

        ir::Declaration::new_ff(clock, reset, statements)
    }
}

impl Conv<&AlwaysCombDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AlwaysCombDeclaration) -> Self {
        let statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
        let statements = statements.0;
        ir::Declaration::new_comb(statements)
    }
}

impl Conv<&VarDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &VarDeclaration) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;

            let Ok(r#type) = x.r#type.to_ir_type(context) else {
                // TODO type evaluation failed error
                return ir::Declaration::Null;
            };

            let mut values = vec![];
            for _ in 0..r#type.total_array() {
                values.push(Value::new_x(r#type.total_width(), false));
            }
            let variable = Variable::new(path.clone(), kind, values, &r#type.array);

            let typed_value = TypedValue::from_type(r#type);
            context.insert_variable(path, typed_value, variable);
        }
        ir::Declaration::Null
    }
}

impl Conv<&LetDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &LetDeclaration) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Variable(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind = VarKind::Variable;

            let Ok(r#type) = x.r#type.to_ir_type(context) else {
                // TODO type evaluation failed error
                return ir::Declaration::Null;
            };

            let mut values = vec![];
            for _ in 0..r#type.total_array() {
                values.push(Value::new_x(r#type.total_width(), false));
            }
            let variable = Variable::new(path.clone(), kind, values, &r#type.array);

            let typed_value = TypedValue::from_type(r#type.clone());
            let id = context.insert_variable(path, typed_value, variable);

            let (_, expr) = eval_expr(context, Some(r#type), &value.expression);
            let statement = ir::Statement::Assign(ir::AssignStatement {
                dst: id,
                index: VarIndex::default(),
                select: vec![],
                expr,
            });

            ir::Declaration::new_comb(vec![statement])
        } else {
            ir::Declaration::Null
        }
    }
}

impl Conv<&ConstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &ConstDeclaration) -> Self {
        if let Ok(symbol) = symbol_table::resolve(value.identifier.as_ref())
            && let SymbolKind::Parameter(x) = symbol.found.kind
        {
            let path = VarPath::new(symbol.found.token.text);
            let kind: VarKind = (&x.kind).into();

            let Ok(r#type) = x.r#type.to_ir_type(context) else {
                // TODO type evaluation failed error
                return ir::Declaration::Null;
            };

            let Some(expr) = &x.value else {
                context.insert_error(AnalyzerError::missing_default_argument(
                    &value.identifier.text().to_string(),
                    &value.identifier.as_ref().into(),
                ));
                return ir::Declaration::Null;
            };

            let (typed_value, _) = eval_expr(context, Some(r#type), expr);

            let Some(value) = typed_value.get_value() else {
                // TODO default value evaluation failed
                return ir::Declaration::Null;
            };

            // TODO for param array
            let variable = Variable::new(path.clone(), kind, vec![value], &[]);
            context.insert_variable(path, typed_value.clone(), variable);
        }
        ir::Declaration::Null
    }
}

impl Conv<&AssignDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &AssignDeclaration) -> Self {
        match value.assign_destination.as_ref() {
            AssignDestination::HierarchicalIdentifier(x) => {
                let ident = x.hierarchical_identifier.as_ref();
                let dst: VarPathIndex = Conv::conv(context, ident);
                let (dst, index) = dst.into();

                if let Some((dst, mut dst_typed_value)) = context.find_path(&dst) {
                    let (index, select) = index.split(dst_typed_value.r#type.array.len());
                    dst_typed_value.r#type.array.drain(0..index.dimension());
                    let (_, expr) =
                        eval_expr(context, Some(dst_typed_value.r#type), &value.expression);
                    let statement = ir::Statement::Assign(ir::AssignStatement {
                        dst,
                        index,
                        select,
                        expr,
                    });
                    ir::Declaration::Comb(ir::CombDeclaration {
                        statements: vec![statement],
                    })
                } else {
                    ir::Declaration::Null
                }
            }
            AssignDestination::LBraceAssignConcatenationListRBrace(_) => {
                // TODO concatenation assignment
                ir::Declaration::Null
            }
        }
    }
}

impl Conv<&FunctionDeclaration> for () {
    fn conv(context: &mut Context, value: &FunctionDeclaration) -> Self {
        context.affiliation.push(Affiliation::Function);

        if let Some(x) = &value.function_declaration_opt0
            && let Some(x) = &x.port_declaration.port_declaration_opt
        {
            let items: Vec<_> = x.port_declaration_list.as_ref().into();
            for item in items {
                let _: ir::Declaration = Conv::conv(context, &item);
            }
        }

        context.affiliation.pop();
    }
}

impl Conv<&EnumDeclaration> for () {
    fn conv(context: &mut Context, value: &EnumDeclaration) -> Self {
        check_enum(context, value);
    }
}

impl Conv<&InitialDeclaration> for () {
    fn conv(context: &mut Context, value: &InitialDeclaration) -> Self {
        let _statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
    }
}

impl Conv<&FinalDeclaration> for () {
    fn conv(context: &mut Context, value: &FinalDeclaration) -> Self {
        let _statements: ir::StatementBlock = Conv::conv(context, value.statement_block.as_ref());
    }
}

impl Conv<&InstDeclaration> for ir::Declaration {
    fn conv(context: &mut Context, value: &InstDeclaration) -> Self {
        let value = value.component_instantiation.as_ref();

        if let Ok(symbol) = symbol_table::resolve(value.scoped_identifier.as_ref())
            && let SymbolKind::Module(ref x) = symbol.found.kind
        {
            let name = symbol.found.token.text;
            let parameters = symbol.found.kind.get_parameters();

            let mut sig = InstanceSignature::new(symbol.found.id);

            let params = get_overridden_params(context, value);
            for x in parameters {
                let path = VarPath::new(x.name);
                if let Some(value) = params.get(&path) {
                    sig.add_param(x.name, value.clone());
                }
            }
            context.overrides = params;

            if let Some(component) = context.instance_history.get(&sig) {
                return ir::Declaration::Inst(ir::InstDeclaration { name, component });
            } else if let Err(x) = context.instance_history.push(sig.clone()) {
                let token: TokenRange = value.identifier.as_ref().into();
                match x {
                    InstanceHistoryError::ExceedDepthLimit => {
                        context.insert_error(AnalyzerError::exceed_limit(
                            "hierarchy depth limit",
                            &token,
                        ));
                    }
                    InstanceHistoryError::ExceedTotalLimit => {
                        context.insert_error(AnalyzerError::exceed_limit(
                            "total instance limit",
                            &token,
                        ));
                    }
                    InstanceHistoryError::InfiniteRecursion => {
                        context.insert_error(AnalyzerError::infinite_recursion(&token));
                    }
                }
            } else {
                let definition = definition_table::get(x.definition).unwrap();
                let Definition::Module(x) = definition else {
                    unreachable!()
                };

                let component: ir::Module = Conv::conv(context, &x);
                let component = ir::Component::Module(component);
                context.instance_history.set(&sig, component.clone());
                context.instance_history.pop();

                return ir::Declaration::Inst(ir::InstDeclaration { name, component });
            }
        }

        ir::Declaration::Null
    }
}

impl Conv<&ModportDeclaration> for () {
    fn conv(context: &mut Context, value: &ModportDeclaration) -> Self {
        context.affiliation.push(Affiliation::Modport);

        if let Some(x) = &value.modport_declaration_opt {
            let items: Vec<_> = x.modport_list.as_ref().into();
            for item in &items {
                check_modport(context, item);
            }
        }

        context.affiliation.pop();
    }
}
