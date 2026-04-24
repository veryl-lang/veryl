use crate::namespace::Namespace;
use crate::symbol::{
    Affiliation, ClockDomain, Direction, DocComment, FunctionProperty, Port, PortProperty, Symbol,
    SymbolKind, TbComponentKind, TbComponentProperty, Type, TypeKind,
};
use crate::symbol_table::SymbolTable;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken};

fn insert_method(
    symbol_table: &mut SymbolTable,
    parent_ns: &Namespace,
    name: &str,
    port_defs: &[(&str, Direction)],
) {
    let func_token = Token::new(name, 0, 0, 0, 0, TokenSource::Builtin);
    let mut func_ns = parent_ns.clone();
    func_ns.push(func_token.text);

    let mut ports = Vec::new();
    for (port_name, direction) in port_defs {
        let port_token = Token::new(port_name, 0, 0, 0, 0, TokenSource::Builtin);
        let port_type = Type {
            modifier: vec![],
            kind: TypeKind::Any,
            width: vec![],
            array: vec![],
            array_type: None,
            is_const: false,
            token: TokenRange::default(),
        };
        let port_prop = PortProperty {
            token: port_token,
            r#type: port_type,
            direction: *direction,
            prefix: None,
            suffix: None,
            clock_domain: ClockDomain::None,
            default_value: None,
            is_proto: false,
        };
        let port_symbol = Symbol::new(
            &port_token,
            SymbolKind::Port(port_prop),
            &func_ns,
            false,
            DocComment::default(),
        );
        if let Some(id) = symbol_table.insert(&port_token, port_symbol) {
            ports.push(Port {
                token: VerylToken::new(port_token),
                symbol: id,
            });
        }
    }

    let func_prop = FunctionProperty {
        affiliation: Affiliation::Module,
        range: TokenRange::default(),
        generic_parameters: vec![],
        generic_consts: vec![],
        generic_references: vec![],
        ports,
        ret: None,
        reference_paths: vec![],
        constantable: None,
        definition: None,
    };
    let func_symbol = Symbol::new(
        &func_token,
        SymbolKind::Function(func_prop),
        parent_ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&func_token, func_symbol);
}

pub fn insert_symbols(symbol_table: &mut SymbolTable, namespace: &Namespace) {
    let mut ns = namespace.clone();

    // Push into $tb namespace (already created by DEFINED_NAMESPACES)
    let tb_token = Token::new("$tb", 0, 0, 0, 0, TokenSource::Builtin);
    ns.push(tb_token.text);

    // $tb::clock_gen
    let clock_token = Token::new("clock_gen", 0, 0, 0, 0, TokenSource::Builtin);
    let clock_symbol = Symbol::new(
        &clock_token,
        SymbolKind::TbComponent(TbComponentProperty {
            kind: TbComponentKind::ClockGen,
        }),
        &ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&clock_token, clock_symbol);

    {
        let mut clock_ns = ns.clone();
        clock_ns.push(clock_token.text);
        insert_method(
            symbol_table,
            &clock_ns,
            "next",
            &[("count", Direction::Input)],
        );
    }

    // $tb::reset_gen
    let reset_token = Token::new("reset_gen", 0, 0, 0, 0, TokenSource::Builtin);
    let reset_symbol = Symbol::new(
        &reset_token,
        SymbolKind::TbComponent(TbComponentProperty {
            kind: TbComponentKind::ResetGen,
        }),
        &ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&reset_token, reset_symbol);

    {
        let mut reset_ns = ns.clone();
        reset_ns.push(reset_token.text);
        insert_method(
            symbol_table,
            &reset_ns,
            "assert",
            &[("count", Direction::Input)],
        );
    }

    ns.pop();
}
