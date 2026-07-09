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
        is_proto: false,
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

/// Registers `$comp::<name>` symbols for the user-defined components
/// declared in `[[components]]` of Veryl.toml. Unlike the builtin `$tb`
/// symbols this runs at `Analyzer::new` (when metadata is available), so it
/// inserts through the global symbol table. Method symbols are not
/// registered; the component interface is only known at simulator load time.
pub fn insert_external_components(names: &[&str]) {
    let mut ns = Namespace::new();
    let component_token = Token::new("$comp", 0, 0, 0, 0, TokenSource::Builtin);
    ns.push(component_token.text);

    for name in names {
        let token = Token::new(name, 0, 0, 0, 0, TokenSource::Builtin);
        let symbol = Symbol::new(
            &token,
            SymbolKind::TbComponent(TbComponentProperty {
                kind: TbComponentKind::External(token.text),
            }),
            &ns,
            true,
            DocComment::default(),
        );
        let _ = crate::symbol_table::insert(&token, symbol);
    }
}

/// Registers `$comp::<project>::<name>` symbols for the components
/// provided by a dependency. The resolution key carried by
/// `TbComponentKind::External` is the composite `"<project>::<name>"`,
/// which is also the key of the built library table; the project's own
/// component keys never contain `::`, so the two sets cannot collide.
pub fn insert_dependency_components(project: &str, names: &[&str]) {
    let mut ns = Namespace::new();
    let component_token = Token::new("$comp", 0, 0, 0, 0, TokenSource::Builtin);
    ns.push(component_token.text);

    let project_token = Token::new(project, 0, 0, 0, 0, TokenSource::Builtin);
    let project_symbol = Symbol::new(
        &project_token,
        SymbolKind::Namespace,
        &ns,
        true,
        DocComment::default(),
    );
    let _ = crate::symbol_table::insert(&project_token, project_symbol);
    ns.push(project_token.text);

    for name in names {
        let token = Token::new(name, 0, 0, 0, 0, TokenSource::Builtin);
        let key = veryl_parser::resource_table::insert_str(&format!("{project}::{name}"));
        let symbol = Symbol::new(
            &token,
            SymbolKind::TbComponent(TbComponentProperty {
                kind: TbComponentKind::External(key),
            }),
            &ns,
            true,
            DocComment::default(),
        );
        let _ = crate::symbol_table::insert(&token, symbol);
    }
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

    // $tb::file
    let file_token = Token::new("file", 0, 0, 0, 0, TokenSource::Builtin);
    let file_symbol = Symbol::new(
        &file_token,
        SymbolKind::TbComponent(TbComponentProperty {
            kind: TbComponentKind::File,
        }),
        &ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&file_token, file_symbol);

    {
        let mut file_ns = ns.clone();
        file_ns.push(file_token.text);
        // `write` is variadic, so it is registered port-less; `tb_method_call`
        // intercepts the call and handles its arguments.
        insert_method(
            symbol_table,
            &file_ns,
            "open",
            &[("name", Direction::Input)],
        );
        insert_method(
            symbol_table,
            &file_ns,
            "append",
            &[("name", Direction::Input)],
        );
        insert_method(symbol_table, &file_ns, "write", &[]);
        insert_method(symbol_table, &file_ns, "close", &[]);
        insert_method(symbol_table, &file_ns, "flush", &[]);
    }

    ns.pop();
}
