use crate::emitter::{SymbolContext, resolve_generic_path, symbol_string};
use std::collections::HashMap;
use veryl_analyzer::attribute::ExpandItem;
use veryl_analyzer::attribute_table;
use veryl_analyzer::evaluator::Evaluator;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::Direction as SymDirection;
use veryl_analyzer::symbol::Type as SymType;
use veryl_analyzer::symbol::{
    GenericMap, GenericTables, Port, Symbol, SymbolId, SymbolKind, VariableProperty,
};
use veryl_analyzer::symbol_table;
use veryl_parser::resource_table::StrId;
use veryl_parser::veryl_grammar_trait::*;
use veryl_parser::veryl_token::{Token, VerylToken};

pub struct ExpandModportConnection {
    pub port_target: VerylToken,
    pub interface_target: VerylToken,
}

pub struct ExpandModportConnections {
    pub connections: Vec<ExpandModportConnection>,
}

impl ExpandModportConnections {
    fn new(
        port: &Port,
        modport: &Symbol,
        interface_name: &VerylToken,
        array_index: &[isize],
    ) -> Self {
        let connections: Vec<_> = collect_modport_member_variables(modport)
            .iter()
            .map(|(variable_token, _variable, _direction)| {
                let (port_target, interface_target) = if array_index.is_empty() {
                    (
                        format!("__{}_{}", port.name(), variable_token),
                        format!("{interface_name}.{variable_token}"),
                    )
                } else {
                    let index: Vec<_> = array_index.iter().map(|x| format!("{x}")).collect();
                    let select: Vec<_> = array_index.iter().map(|x| format!("[{x}]")).collect();
                    (
                        format!("__{}_{}_{}", port.name(), index.join("_"), variable_token),
                        format!("{}{}.{}", interface_name, select.join(""), variable_token),
                    )
                };
                ExpandModportConnection {
                    port_target: port.token.replace(&port_target),
                    interface_target: interface_name.replace(&interface_target),
                }
            })
            .collect();
        Self { connections }
    }
}

pub struct ExpandModportConnectionsTableEntry {
    id: StrId,
    index: usize,
    pub connections: Vec<ExpandModportConnections>,
}

pub struct ExpandModportConnectionsTable {
    entries: Vec<ExpandModportConnectionsTableEntry>,
}

impl ExpandModportConnectionsTable {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn create_from_inst_ports(
        defined_ports: &[Port],
        inst_ports: &[InstPortItem],
        generic_map: &[GenericMap],
        namespace: &Namespace,
    ) -> Self {
        let connected_ports: HashMap<StrId, Option<&VerylToken>> = inst_ports
            .iter()
            .map(|x| {
                let token = if let Some(ref x) = x.inst_port_item_opt {
                    x.expression.unwrap_identifier().map(|x| x.identifier())
                } else {
                    None
                };
                (x.identifier.identifier_token.token.text, token)
            })
            .collect();

        let mut ret = ExpandModportConnectionsTable::new();
        ret.expand(
            defined_ports,
            &connected_ports,
            generic_map,
            namespace,
            false,
        );
        ret
    }

    pub fn create_from_argument_list(
        defined_ports: &[Port],
        argument_list: &ArgumentList,
        generic_map: &[GenericMap],
        namespace: &Namespace,
    ) -> Self {
        let mut list: Vec<_> = argument_list
            .argument_list_list
            .iter()
            .map(|x| x.argument_item.clone())
            .collect();
        list.insert(0, argument_list.argument_item.clone());

        let connected_ports: HashMap<StrId, Option<&VerylToken>> = list
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                if i < defined_ports.len() && arg.argument_item_opt.is_none() {
                    let lhs_token = defined_ports[i].token.token.text;
                    let rhs_token = arg
                        .argument_expression
                        .expression
                        .unwrap_identifier()
                        .map(|x| x.identifier());
                    (Some(lhs_token), rhs_token)
                } else {
                    let lhs_token = arg
                        .argument_expression
                        .expression
                        .unwrap_identifier()
                        .map(|x| x.identifier().token.text);
                    let rhs_token = if let Some(ref rhs) = arg.argument_item_opt {
                        rhs.expression.unwrap_identifier().map(|x| x.identifier())
                    } else {
                        None
                    };
                    (lhs_token, rhs_token)
                }
            })
            .filter(|(lhs, _)| lhs.is_some())
            .map(|(lhs, rhs)| (lhs.unwrap(), rhs))
            .collect();

        let mut ret = ExpandModportConnectionsTable::new();
        ret.expand(
            defined_ports,
            &connected_ports,
            generic_map,
            namespace,
            true,
        );
        ret
    }

    fn expand(
        &mut self,
        defined_ports: &[Port],
        connected_ports: &HashMap<StrId, Option<&VerylToken>>,
        generic_map: &[GenericMap],
        namespace: &Namespace,
        in_function: bool,
    ) {
        for (modport, port, index) in collect_modports(defined_ports, namespace) {
            if !(in_function || attribute_table::is_expand(&port.token.token, ExpandItem::Modport))
            {
                continue;
            }

            let connected_interface = connected_ports
                .get(&port.name())
                .map(|x| x.unwrap_or(&port.token))
                .unwrap();
            let property = port.property();
            let array_size = evaluate_array_size(&property.r#type.array, generic_map);
            let array_index = expand_array_index(&array_size, &[]);
            let connections: Vec<_> = if array_index.is_empty() {
                vec![ExpandModportConnections::new(
                    &port,
                    &modport,
                    connected_interface,
                    &[],
                )]
            } else {
                array_index
                    .iter()
                    .map(|index| {
                        ExpandModportConnections::new(&port, &modport, connected_interface, index)
                    })
                    .collect()
            };

            let entry = ExpandModportConnectionsTableEntry {
                id: port.name(),
                index,
                connections,
            };
            self.entries.push(entry);
        }
    }

    pub fn remove(&mut self, token: &VerylToken) -> Option<ExpandModportConnectionsTableEntry> {
        let index = self.entries.iter().position(|x| x.id == token.token.text)?;
        Some(self.entries.remove(index))
    }

    pub fn pop_front(&mut self, port_index: usize) -> Option<ExpandModportConnectionsTableEntry> {
        if self
            .entries
            .first()
            .map(|x| x.index == port_index)
            .unwrap_or(false)
        {
            Some(self.entries.remove(0))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExpandedModportPort {
    pub id: StrId,
    pub array_index: Vec<isize>,
    pub identifier: VerylToken,
    pub r#type: SymType,
    pub interface_target: VerylToken,
    pub direction: SymDirection,
    pub direction_token: VerylToken,
}

#[derive(Clone, Debug)]
pub struct ExpandedModportPorts {
    pub ports: Vec<ExpandedModportPort>,
}

impl ExpandedModportPorts {
    fn new(port: &Port, modport: &Symbol, array_index: &[isize]) -> Self {
        let ports: Vec<_> = collect_modport_member_variables(modport)
            .iter()
            .map(|(variable_token, variable, direction)| {
                let (port_name, interface_target) = if array_index.is_empty() {
                    (
                        format!("__{}_{}", port.name(), variable_token),
                        format!("{}.{}", port.name(), variable_token),
                    )
                } else {
                    let index: Vec<_> = array_index.iter().map(|x| format!("{x}")).collect();
                    let select: Vec<_> = array_index.iter().map(|x| format!("[{x}]")).collect();
                    (
                        format!("__{}_{}_{}", port.name(), index.join("_"), variable_token),
                        format!("{}{}.{}", port.name(), select.join(""), variable_token),
                    )
                };
                let direction_token = if matches!(direction, SymDirection::Input) {
                    port.token.replace("input")
                } else {
                    port.token.replace("output")
                };
                ExpandedModportPort {
                    id: variable_token.text,
                    array_index: array_index.to_vec(),
                    identifier: port.token.replace(&port_name),
                    r#type: variable.r#type.clone(),
                    interface_target: port.token.replace(&interface_target),
                    direction: *direction,
                    direction_token,
                }
            })
            .collect();
        Self { ports }
    }
}

#[derive(Clone, Debug)]
pub struct ExpandedModportPortTableEntry {
    id: StrId,
    pub identifier: VerylToken,
    pub interface_name: VerylToken,
    pub array_size: Vec<isize>,
    pub generic_maps: Vec<GenericMap>,
    pub ports: Vec<ExpandedModportPorts>,
}

pub struct ExpandedModportPortTable {
    entries: Vec<ExpandedModportPortTableEntry>,
}

impl ExpandedModportPortTable {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn create(
        defined_ports: &[Port],
        generic_map: &[GenericMap],
        namespace_token: &VerylToken,
        namespace: &Namespace,
        in_function: bool,
        context: &SymbolContext,
    ) -> Self {
        let mut ret = ExpandedModportPortTable::new();
        ret.expand(
            defined_ports,
            generic_map,
            namespace_token,
            namespace,
            in_function,
            context,
        );
        ret
    }

    fn expand(
        &mut self,
        defined_ports: &[Port],
        generic_map: &[GenericMap],
        namespace_token: &VerylToken,
        namespace: &Namespace,
        in_function: bool,
        context: &SymbolContext,
    ) {
        for (modport, port, _) in collect_modports(defined_ports, namespace) {
            if !(in_function || attribute_table::is_expand(&port.token.token, ExpandItem::Modport))
            {
                continue;
            }

            let Some((interface_symbol, interface_path, interface_tables)) =
                resolve_interface(&port, namespace, generic_map)
            else {
                unreachable!()
            };

            let property = port.property();
            let array_size = evaluate_array_size(&property.r#type.array, generic_map);
            let array_index = expand_array_index(&array_size, &[]);
            let interface_name = {
                let text = symbol_string(
                    namespace_token,
                    &interface_symbol,
                    &interface_symbol.namespace,
                    &interface_path,
                    &interface_tables,
                    context,
                    1,
                );
                port.token.replace(&text)
            };

            let ports = if array_index.is_empty() {
                vec![ExpandedModportPorts::new(&port, &modport, &[])]
            } else {
                array_index
                    .iter()
                    .map(|index| ExpandedModportPorts::new(&port, &modport, index))
                    .collect()
            };

            let entry = ExpandedModportPortTableEntry {
                id: port.name(),
                identifier: port.token.clone(),
                interface_name,
                generic_maps: interface_symbol.generic_maps(),
                array_size,
                ports,
            };
            self.entries.push(entry);
        }
    }

    pub fn get(&self, token: &Token) -> Option<ExpandedModportPortTableEntry> {
        self.entries.iter().find(|x| x.id == token.text).cloned()
    }

    pub fn get_modport_member(
        &self,
        modport_token: &Token,
        member_token: &Token,
        array_index: &[isize],
    ) -> Option<ExpandedModportPort> {
        let entry = self.entries.iter().find(|x| x.id == modport_token.text)?;
        entry
            .ports
            .iter()
            .flat_map(|x| x.ports.iter())
            .find(|x| x.id == member_token.text && x.array_index == array_index)
            .cloned()
    }

    pub fn drain(&mut self) -> Vec<ExpandedModportPortTableEntry> {
        self.entries.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn collect_modports(ports: &[Port], namespace: &Namespace) -> Vec<(Symbol, Port, usize)> {
    ports
        .iter()
        .enumerate()
        .filter_map(|(i, port)| {
            let property = port.property();
            if let Some((_, Some(symbol))) = property.r#type.trace_user_defined(Some(namespace)) {
                if matches!(symbol.kind, SymbolKind::Modport(_)) {
                    Some((symbol, port.clone(), i))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

fn evaluate_array_size(array_size: &[Expression], generic_map: &[GenericMap]) -> Vec<isize> {
    let mut evaluator = Evaluator::new(generic_map);
    array_size
        .iter()
        .filter_map(|x| evaluator.expression(x).get_value())
        .collect()
}

fn expand_array_index(array_size: &[isize], array_index: &[Vec<isize>]) -> Vec<Vec<isize>> {
    if array_size.is_empty() {
        return array_index.to_vec();
    }

    let mut array_size = array_size.to_owned();
    let size = array_size.pop().unwrap();

    let mut ret: Vec<_> = Vec::new();
    for s in 0..size {
        if array_index.is_empty() {
            ret.push(vec![s]);
        } else {
            let mut index: Vec<_> = array_index
                .iter()
                .map(|x| {
                    let mut x = x.clone();
                    x.insert(0, s);
                    x
                })
                .collect();
            ret.append(&mut index);
        }
    }

    if array_size.is_empty() {
        ret
    } else {
        expand_array_index(&array_size, &ret)
    }
}

fn collect_modport_member_variables(
    symbol: &Symbol,
) -> Vec<(Token, VariableProperty, SymDirection)> {
    let SymbolKind::Modport(modport) = &symbol.kind else {
        unreachable!()
    };

    modport
        .members
        .iter()
        .filter_map(|member| {
            if let SymbolKind::ModportVariableMember(member) =
                symbol_table::get(*member).unwrap().kind
            {
                let variable_symbol = symbol_table::get(member.variable).unwrap();
                if let SymbolKind::Variable(variable) = variable_symbol.kind {
                    Some((variable_symbol.token, variable, member.direction))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

fn resolve_interface(
    port: &Port,
    namespace: &Namespace,
    generic_map: &[GenericMap],
) -> Option<(Symbol, Vec<SymbolId>, GenericTables)> {
    let property = port.property();
    let (user_defined, _) = property.r#type.trace_user_defined(Some(namespace))?;

    let mut path = user_defined.get_user_defined()?.path.clone();
    path.paths.pop(); // remove modport path

    let (result, _) = resolve_generic_path(&path, namespace, Some(&generic_map.to_vec()));
    result
        .ok()
        .map(|x| (x.found, x.full_path, x.generic_tables))
}
