use crate::namespace::Namespace;
use crate::symbol::{
    Affiliation, ClockDomain, Direction, DocComment, FunctionProperty, GenericBoundKind,
    GenericParameterProperty, Port, PortProperty, Symbol, SymbolKind, TbComponentKind,
    TbComponentProperty, Type, TypeKind, UserDefinedType,
};
use crate::symbol_path::{GenericSymbol, GenericSymbolPath, GenericSymbolPathKind};
use crate::symbol_table::SymbolTable;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::{Token, TokenSource, VerylToken};

/// A builtin `Type` with no width/array (widths for fixed types are derived
/// from the `TypeKind` itself).
fn builtin_type(kind: TypeKind) -> Type {
    Type {
        modifier: vec![],
        kind,
        width: vec![],
        array: vec![],
        array_type: None,
        is_const: false,
        token: TokenRange::default(),
    }
}

/// A `Type` referring to the generic parameter `T` (`base` token) of
/// `$tb::random`. Left unresolved (`symbol: None`) so the return type is
/// resolved through the generic map at the call site, like any `-> T`.
fn generic_param_type(base: Token) -> Type {
    let range: TokenRange = base.into();
    let path = GenericSymbolPath {
        paths: vec![GenericSymbol {
            base,
            arguments: vec![],
        }],
        kind: GenericSymbolPathKind::Identifier,
        range,
    };
    Type {
        modifier: vec![],
        kind: TypeKind::UserDefined(UserDefinedType { path, symbol: None }),
        width: vec![],
        array: vec![],
        array_type: None,
        is_const: false,
        token: range,
    }
}

fn insert_method(
    symbol_table: &mut SymbolTable,
    parent_ns: &Namespace,
    name: &str,
    port_defs: &[(&str, Direction)],
    ret: Option<Type>,
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
        ret,
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
                generic_parameters: vec![],
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
                generic_parameters: vec![],
            }),
            &ns,
            true,
            DocComment::default(),
        );
        let _ = crate::symbol_table::insert(&token, symbol);
    }
}

/// Inserts a non-generic `$tb::<name>` component symbol and returns its inner
/// namespace, for registering methods under it.
fn insert_component(
    symbol_table: &mut SymbolTable,
    tb_ns: &Namespace,
    name: &str,
    kind: TbComponentKind,
) -> Namespace {
    let token = Token::new(name, 0, 0, 0, 0, TokenSource::Builtin);
    let symbol = Symbol::new(
        &token,
        SymbolKind::TbComponent(TbComponentProperty {
            kind,
            generic_parameters: vec![],
        }),
        tb_ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&token, symbol);
    let mut ns = tb_ns.clone();
    ns.push(token.text);
    ns
}

fn insert_clock_gen(symbol_table: &mut SymbolTable, tb_ns: &Namespace) {
    let ns = insert_component(symbol_table, tb_ns, "clock_gen", TbComponentKind::ClockGen);
    insert_method(
        symbol_table,
        &ns,
        "next",
        &[("count", Direction::Input)],
        None,
    );
}

fn insert_reset_gen(symbol_table: &mut SymbolTable, tb_ns: &Namespace) {
    let ns = insert_component(symbol_table, tb_ns, "reset_gen", TbComponentKind::ResetGen);
    insert_method(
        symbol_table,
        &ns,
        "assert",
        &[("count", Direction::Input)],
        None,
    );
}

fn insert_file(symbol_table: &mut SymbolTable, tb_ns: &Namespace) {
    let ns = insert_component(symbol_table, tb_ns, "file", TbComponentKind::File);
    // `write` is variadic, so it is registered port-less; `tb_method_call`
    // intercepts the call and handles its arguments.
    insert_method(
        symbol_table,
        &ns,
        "open",
        &[("name", Direction::Input)],
        None,
    );
    insert_method(
        symbol_table,
        &ns,
        "append",
        &[("name", Direction::Input)],
        None,
    );
    insert_method(symbol_table, &ns, "write", &[], None);
    insert_method(symbol_table, &ns, "close", &[], None);
    insert_method(symbol_table, &ns, "flush", &[], None);
}

fn insert_random(symbol_table: &mut SymbolTable, tb_ns: &Namespace) {
    let random_token = Token::new("random", 0, 0, 0, 0, TokenSource::Builtin);
    let mut ns = tb_ns.clone();
    ns.push(random_token.text);

    // Synthesize the generic type parameter `T` inside the `$tb::random`
    // namespace so member methods can return `-> T`, resolved through the
    // normal generic pipeline at the call site.
    let t_token = Token::new("T", 0, 0, 0, 0, TokenSource::Builtin);
    let t_symbol = Symbol::new(
        &t_token,
        SymbolKind::GenericParameter(GenericParameterProperty {
            bound: GenericBoundKind::Type,
            default_value: None,
        }),
        &ns,
        false,
        DocComment::default(),
    );
    let Some(t_id) = symbol_table.insert(&t_token, t_symbol) else {
        return;
    };

    let random_symbol = Symbol::new(
        &random_token,
        SymbolKind::TbComponent(TbComponentProperty {
            kind: TbComponentKind::Random,
            generic_parameters: vec![t_id],
        }),
        tb_ns,
        true,
        DocComment::default(),
    );
    let _ = symbol_table.insert(&random_token, random_symbol);

    // `seed`/`get`/`get_range` take value arguments handled by
    // `tb_method_call`; the ports are placeholders. Return types drive the
    // static typing: `get`/`get_range` return `T`, `get_seed`
    // returns a 64-bit unsigned, `seed` returns nothing.
    insert_method(
        symbol_table,
        &ns,
        "seed",
        &[("value", Direction::Input)],
        None,
    );
    insert_method(
        symbol_table,
        &ns,
        "get",
        &[],
        Some(generic_param_type(t_token)),
    );
    insert_method(
        symbol_table,
        &ns,
        "get_range",
        &[("min", Direction::Input), ("max", Direction::Input)],
        Some(generic_param_type(t_token)),
    );
    insert_method(
        symbol_table,
        &ns,
        "get_seed",
        &[],
        Some(builtin_type(TypeKind::U64)),
    );
}

pub fn insert_symbols(symbol_table: &mut SymbolTable, namespace: &Namespace) {
    let mut tb_ns = namespace.clone();

    // Push into $tb namespace (already created by DEFINED_NAMESPACES)
    let tb_token = Token::new("$tb", 0, 0, 0, 0, TokenSource::Builtin);
    tb_ns.push(tb_token.text);

    insert_clock_gen(symbol_table, &tb_ns);
    insert_reset_gen(symbol_table, &tb_ns);
    insert_file(symbol_table, &tb_ns);
    insert_random(symbol_table, &tb_ns);
}
