use crate::HashMap;
use crate::analyzer_error::AnalyzerError;
use crate::symbol::{Direction, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::SymbolPath;
use crate::symbol_table;
use crate::var_ref::{ExpressionTargetType, VarRefPath, VarRefPathItem};
use std::cell::RefCell;
use std::convert::TryFrom;
use veryl_parser::resource_table::TokenId;
use veryl_parser::veryl_grammar_trait::{
    Expression, ExpressionIdentifier, HierarchicalIdentifier, Select,
};
use veryl_parser::veryl_token::Token;

#[derive(Clone, Debug)]
pub struct ConnectModportOperand {
    pub id: SymbolId,
    pub base_path: VarRefPath,
    pub via_instance: bool,
}

impl ConnectModportOperand {
    pub fn get_ports(&self) -> Vec<(Symbol, Direction)> {
        let modport = symbol_table::get(self.id).unwrap();
        if let SymbolKind::Modport(x) = modport.kind {
            x.members
                .iter()
                .map(|x| symbol_table::get(*x).unwrap())
                .filter(|x| matches!(x.kind, SymbolKind::ModportVariableMember(_)))
                .map(|x| {
                    if let SymbolKind::ModportVariableMember(x) = &x.kind {
                        let var_symbol = symbol_table::get(x.variable).unwrap();
                        (var_symbol, x.direction)
                    } else {
                        unreachable!()
                    }
                })
                .collect()
        } else {
            unreachable!()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectExpressionOperand {
    pub expression: Expression,
}

#[derive(Clone, Debug)]
pub enum ConnectOperand {
    Modport(ConnectModportOperand),
    Expression(ConnectExpressionOperand),
}

#[derive(Clone, Debug)]
pub struct ConnectOperation {
    pub lhs: ConnectOperand,
    pub rhs: ConnectOperand,
}

impl ConnectOperation {
    pub fn is_lhs_instance(&self) -> bool {
        if let ConnectOperand::Modport(x) = &self.lhs {
            x.via_instance
        } else {
            false
        }
    }

    pub fn is_rhs_instance(&self) -> bool {
        if let ConnectOperand::Modport(x) = &self.rhs {
            x.via_instance
        } else {
            false
        }
    }

    pub fn get_connection_pairs(&self) -> Vec<(Symbol, Direction, Symbol, Direction)> {
        if let (ConnectOperand::Modport(lhs), ConnectOperand::Modport(rhs)) = (&self.lhs, &self.rhs)
        {
            let lhs_ports = lhs.get_ports();
            let mut rhs_ports = rhs.get_ports();
            let mut ret = Vec::new();

            for (lhs_symbol, lhs_direction) in lhs_ports {
                let connectable_direction = match lhs_direction {
                    Direction::Input => Direction::Output,
                    Direction::Output => Direction::Input,
                    Direction::Inout => Direction::Inout,
                    _ => unreachable!(),
                };

                for i in 0..rhs_ports.len() {
                    if rhs_ports[i].0.token.text == lhs_symbol.token.text
                        && rhs_ports[i].1 == connectable_direction
                    {
                        let rhs_port = rhs_ports.remove(i);
                        ret.push((lhs_symbol, lhs_direction, rhs_port.0, rhs_port.1));
                        break;
                    }
                }
            }

            ret
        } else {
            vec![]
        }
    }

    pub fn get_ports_with_expression(&self) -> Option<(Vec<(Symbol, Direction)>, Expression)> {
        if let (ConnectOperand::Modport(lhs), ConnectOperand::Expression(rhs)) =
            (&self.lhs, &self.rhs)
        {
            let lhs_ports: Vec<_> = lhs
                .get_ports()
                .into_iter()
                .filter(|(_, direction)| matches!(direction, Direction::Output | Direction::Inout))
                .collect();
            Some((lhs_ports, rhs.expression.clone()))
        } else {
            None
        }
    }

    pub fn get_assign_paths(&self) -> Vec<VarRefPath> {
        match (&self.lhs, &self.rhs) {
            (ConnectOperand::Modport(lhs), ConnectOperand::Modport(rhs)) => {
                let mut ret = Vec::new();
                for (lhs_symbol, lhs_direction, rhs_symbol, rhs_direction) in
                    self.get_connection_pairs()
                {
                    if matches!(lhs_direction, Direction::Output | Direction::Inout) {
                        let mut path = lhs.base_path.clone();
                        path.push(VarRefPathItem::Identifier {
                            symbol_id: lhs_symbol.id,
                        });
                        ret.push(path);
                    }
                    if matches!(rhs_direction, Direction::Output | Direction::Inout) {
                        let mut path = rhs.base_path.clone();
                        path.push(VarRefPathItem::Identifier {
                            symbol_id: rhs_symbol.id,
                        });
                        ret.push(path);
                    }
                }

                ret
            }
            (ConnectOperand::Modport(lhs), ConnectOperand::Expression(_)) => {
                let (ports, _) = self.get_ports_with_expression().unwrap();
                ports
                    .iter()
                    .map(|(port, _)| {
                        let mut path = lhs.base_path.clone();
                        path.push(VarRefPathItem::Identifier { symbol_id: port.id });
                        path
                    })
                    .collect()
            }
            _ => unreachable!(),
        }
    }

    pub fn get_expression_paths(&self) -> Vec<(VarRefPath, ExpressionTargetType)> {
        let mut ret = Vec::new();

        if let (ConnectOperand::Modport(lhs), ConnectOperand::Modport(rhs)) = (&self.lhs, &self.rhs)
        {
            for (lhs_symbol, lhs_direction, rhs_symbol, rhs_direction) in
                self.get_connection_pairs()
            {
                if matches!(lhs_direction, Direction::Input | Direction::Inout) {
                    let mut path = lhs.base_path.clone();
                    path.push(VarRefPathItem::Identifier {
                        symbol_id: lhs_symbol.id,
                    });

                    if lhs.via_instance {
                        ret.push((path, ExpressionTargetType::Variable));
                    } else if matches!(lhs_direction, Direction::Input) {
                        ret.push((path, ExpressionTargetType::InputPort));
                    } else {
                        ret.push((path, ExpressionTargetType::InoutPort));
                    }
                }
                if matches!(rhs_direction, Direction::Input | Direction::Inout) {
                    let mut path = rhs.base_path.clone();
                    path.push(VarRefPathItem::Identifier {
                        symbol_id: rhs_symbol.id,
                    });

                    if rhs.via_instance {
                        ret.push((path, ExpressionTargetType::Variable));
                    } else if matches!(rhs_direction, Direction::Input) {
                        ret.push((path, ExpressionTargetType::InputPort));
                    } else {
                        ret.push((path, ExpressionTargetType::InoutPort));
                    }
                }
            }
        }

        ret
    }
}

#[derive(Clone, Default, Debug)]
pub struct ConnectOperationTable {
    table: HashMap<TokenId, ConnectOperation>,
}

impl ConnectOperationTable {
    pub fn insert(&mut self, token: &Token, lhs: ConnectOperand, rhs: ConnectOperand) {
        let operation = ConnectOperation { lhs, rhs };
        self.table.insert(token.id, operation);
    }

    pub fn get(&self, token: &Token) -> Option<ConnectOperation> {
        self.table.get(&token.id).cloned()
    }

    pub fn clear(&mut self) {
        self.table.clear()
    }
}

thread_local!(static CONNECT_OPERATION_TABLE: RefCell<ConnectOperationTable> = RefCell::new(ConnectOperationTable::default()));

pub fn insert(token: &Token, lhs: ConnectOperand, rhs: ConnectOperand) {
    CONNECT_OPERATION_TABLE.with(|f| f.borrow_mut().insert(token, lhs, rhs))
}

pub fn get(token: &Token) -> Option<ConnectOperation> {
    CONNECT_OPERATION_TABLE.with(|f| f.borrow().get(token))
}

pub fn clear() {
    CONNECT_OPERATION_TABLE.with(|f| f.borrow_mut().clear())
}

fn mismatch_type(symbol: &Symbol, expected: &str) -> Option<AnalyzerError> {
    let error = AnalyzerError::mismatch_type(
        &symbol.token.to_string(),
        expected,
        &symbol.kind.to_kind_name(),
        &symbol.token.into(),
    );
    Some(error)
}

fn invalid_connect_operand(symbol: &Symbol, reason: &str) -> Option<AnalyzerError> {
    let error = AnalyzerError::invalid_connect_operand(
        &symbol.token.to_string(),
        reason,
        &symbol.token.into(),
    );
    Some(error)
}

fn is_single_element(array: &[Expression], select: &[Select]) -> bool {
    array.len() == select.len()
        && (array.is_empty() || select.iter().all(|x| x.select_opt.is_none()))
}

impl TryFrom<&HierarchicalIdentifier> for ConnectOperand {
    type Error = Option<AnalyzerError>;

    fn try_from(arg: &HierarchicalIdentifier) -> Result<Self, Self::Error> {
        if let Ok(mut base_path) = VarRefPath::try_from(arg) {
            let full_path = &base_path.full_path();
            let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();

            let result = match &symbol.kind {
                SymbolKind::Modport(_) => {
                    // Specify modport via interface instance
                    let instance_pos = full_path.len() - 2;
                    let instance = symbol_table::get(full_path[instance_pos]).unwrap();
                    let array = if let SymbolKind::Instance(x) = &instance.kind {
                        &x.array
                    } else {
                        unreachable!()
                    };
                    let select: Vec<_> = if arg.hierarchical_identifier_list0.len() >= 2 {
                        arg.hierarchical_identifier_list0[instance_pos - 1]
                            .hierarchical_identifier_list0_list
                            .iter()
                            .map(|x| x.select.as_ref().clone())
                            .collect()
                    } else {
                        arg.hierarchical_identifier_list
                            .iter()
                            .map(|x| x.select.as_ref().clone())
                            .collect()
                    };

                    if !is_single_element(array, &select) {
                        let error =
                            invalid_connect_operand(&instance, "it is an array interface instance");
                        return Err(error);
                    }

                    Ok((symbol.id, true))
                }
                SymbolKind::Port(x) => {
                    // Specify modport via port
                    let array = &x.r#type.array;
                    if let TypeKind::UserDefined(x) = &x.r#type.kind {
                        if let Ok(type_symbol) =
                            symbol_table::resolve((&SymbolPath::new(&x.path), &symbol.namespace))
                        {
                            if matches!(type_symbol.found.kind, SymbolKind::Modport(_)) {
                                if !is_single_element(array, &arg.last_select()) {
                                    let error =
                                        invalid_connect_operand(&symbol, "it is an array modport");
                                    return Err(error);
                                }
                                Ok((type_symbol.found.id, false))
                            } else {
                                Err(type_symbol.found.id)
                            }
                        } else {
                            return Err(None);
                        }
                    } else {
                        Err(symbol.id)
                    }
                }
                _ => Err(symbol.id),
            };

            match result {
                Ok((id, via_instance)) => {
                    if via_instance {
                        base_path.pop();
                    }
                    let operand = ConnectModportOperand {
                        id,
                        base_path,
                        via_instance,
                    };
                    Ok(ConnectOperand::Modport(operand))
                }
                Err(id) => {
                    let error_symbol = symbol_table::get(id).unwrap();
                    Err(mismatch_type(&error_symbol, "modport"))
                }
            }
        } else {
            Err(None)
        }
    }
}

impl TryFrom<&ExpressionIdentifier> for ConnectOperand {
    type Error = Option<AnalyzerError>;

    fn try_from(arg: &ExpressionIdentifier) -> Result<Self, Self::Error> {
        if let Ok(mut base_path) = VarRefPath::try_from(arg) {
            let full_path = &base_path.full_path();
            let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();

            let result = match &symbol.kind {
                SymbolKind::Modport(_) => {
                    // Specify modport via interface instance
                    let instance_pos = full_path.len() - 2;
                    let instance = symbol_table::get(full_path[instance_pos]).unwrap();
                    let array = if let SymbolKind::Instance(x) = &instance.kind {
                        &x.array
                    } else {
                        unreachable!()
                    };
                    let select: Vec<_> = if arg.expression_identifier_list0.len() >= 2 {
                        arg.expression_identifier_list0[instance_pos - 1]
                            .expression_identifier_list0_list
                            .iter()
                            .map(|x| x.select.as_ref().clone())
                            .collect()
                    } else {
                        arg.expression_identifier_list
                            .iter()
                            .map(|x| x.select.as_ref().clone())
                            .collect()
                    };

                    if !is_single_element(array, &select) {
                        let error =
                            invalid_connect_operand(&instance, "it is an array interface instance");
                        return Err(error);
                    }

                    Ok((symbol.id, true))
                }
                SymbolKind::Port(x) => {
                    // Specify modport via port
                    let array = &x.r#type.array;
                    if let TypeKind::UserDefined(x) = &x.r#type.kind {
                        if let Ok(type_symbol) =
                            symbol_table::resolve((&SymbolPath::new(&x.path), &symbol.namespace))
                        {
                            if matches!(type_symbol.found.kind, SymbolKind::Modport(_)) {
                                if !is_single_element(array, &arg.last_select()) {
                                    return Err(invalid_connect_operand(
                                        &symbol,
                                        "it is an array modport",
                                    ));
                                }
                                Ok((type_symbol.found.id, false))
                            } else {
                                Err(type_symbol.found.id)
                            }
                        } else {
                            return Err(None);
                        }
                    } else {
                        Err(symbol.id)
                    }
                }
                _ => Err(symbol.id),
            };

            match result {
                Ok((id, via_instance)) => {
                    if via_instance {
                        base_path.pop();
                    }
                    let operand = ConnectModportOperand {
                        id,
                        base_path,
                        via_instance,
                    };
                    Ok(ConnectOperand::Modport(operand))
                }
                Err(id) => {
                    let error_symbol = symbol_table::get(id).unwrap();
                    Err(mismatch_type(&error_symbol, "modport"))
                }
            }
        } else {
            Err(None)
        }
    }
}

impl TryFrom<&Expression> for ConnectOperand {
    type Error = Option<AnalyzerError>;

    fn try_from(arg: &Expression) -> Result<Self, Self::Error> {
        if let Some(exp) = arg.unwrap_identifier() {
            if let Ok(mut base_path) = VarRefPath::try_from(exp) {
                let full_path = &base_path.full_path();
                let symbol = symbol_table::get(*full_path.last().unwrap()).unwrap();
                let id = match &symbol.kind {
                    SymbolKind::Modport(_) => {
                        // Specify modport via interface instance
                        let instance_pos = full_path.len() - 2;
                        let instance = symbol_table::get(full_path[instance_pos]).unwrap();
                        let array = if let SymbolKind::Instance(x) = &instance.kind {
                            &x.array
                        } else {
                            unreachable!()
                        };
                        let select: Vec<_> = if exp.expression_identifier_list0.len() >= 2 {
                            exp.expression_identifier_list0[instance_pos - 1]
                                .expression_identifier_list0_list
                                .iter()
                                .map(|x| x.select.as_ref().clone())
                                .collect()
                        } else {
                            exp.expression_identifier_list
                                .iter()
                                .map(|x| x.select.as_ref().clone())
                                .collect()
                        };

                        if !is_single_element(array, &select) {
                            let error = invalid_connect_operand(
                                &instance,
                                "it is an array interface instance",
                            );
                            return Err(error);
                        }

                        Some((symbol.id, true))
                    }
                    SymbolKind::Port(x) => {
                        // Specify modport via port
                        let array = &x.r#type.array;
                        if let TypeKind::UserDefined(x) = &x.r#type.kind {
                            if let Ok(type_symbol) = symbol_table::resolve((
                                &SymbolPath::new(&x.path),
                                &symbol.namespace,
                            )) {
                                if matches!(type_symbol.found.kind, SymbolKind::Modport(_)) {
                                    if !is_single_element(array, &exp.last_select()) {
                                        let error = invalid_connect_operand(
                                            &symbol,
                                            "it is an array modport",
                                        );
                                        return Err(error);
                                    }
                                    Some((type_symbol.found.id, false))
                                } else {
                                    // type is not modport then it is expression
                                    None
                                }
                            } else {
                                return Err(None);
                            }
                        } else {
                            // type is a built-in type then it is expression
                            None
                        }
                    }
                    SymbolKind::Instance(_) => {
                        let error = mismatch_type(&symbol, "modport or expression");
                        return Err(error);
                    }
                    _ => None,
                };

                if let Some((id, via_instance)) = id {
                    if via_instance {
                        base_path.pop();
                    }
                    let operand = ConnectModportOperand {
                        id,
                        base_path,
                        via_instance,
                    };
                    Ok(ConnectOperand::Modport(operand))
                } else {
                    let operand = ConnectExpressionOperand {
                        expression: arg.clone(),
                    };
                    Ok(ConnectOperand::Expression(operand))
                }
            } else {
                Err(None)
            }
        } else {
            // arg inclues other elements other than expression identifier
            // then it is expression
            let operand = ConnectExpressionOperand {
                expression: arg.clone(),
            };
            Ok(ConnectOperand::Expression(operand))
        }
    }
}
