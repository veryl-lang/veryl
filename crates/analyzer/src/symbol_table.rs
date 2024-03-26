use crate::evaluator::Evaluated;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::symbol::{Direction, DocComment, Symbol, SymbolId, SymbolKind, TypeKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{self, PathId, StrId, TokenId};
use veryl_parser::veryl_grammar_trait as syntax_tree;
use veryl_parser::veryl_token::{Token, TokenSource};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SymbolPath(Vec<StrId>);

impl SymbolPath {
    pub fn new(x: &[StrId]) -> Self {
        Self(x.to_vec())
    }

    pub fn push(&mut self, x: StrId) {
        self.0.push(x)
    }

    pub fn pop(&mut self) -> Option<StrId> {
        self.0.pop()
    }

    pub fn clear(&mut self) {
        self.0.clear()
    }

    pub fn as_slice(&self) -> &[StrId] {
        self.0.as_slice()
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = String::new();
        for path in self.as_slice() {
            text.push_str(&format!("{path} "));
        }
        text.fmt(f)
    }
}

impl From<&[Token]> for SymbolPath {
    fn from(value: &[Token]) -> Self {
        let mut path = Vec::new();
        for x in value {
            path.push(x.text);
        }
        SymbolPath(path)
    }
}

impl From<&Token> for SymbolPath {
    fn from(value: &Token) -> Self {
        let path = vec![value.text];
        SymbolPath(path)
    }
}

impl From<&syntax_tree::Identifier> for SymbolPath {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let path = vec![value.identifier_token.token.text];
        SymbolPath(path)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPath {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let mut path = Vec::new();
        for x in value {
            path.push(x.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let mut path = Vec::new();
        path.push(value.identifier.identifier_token.token.text);
        for x in &value.hierarchical_identifier_list0 {
            path.push(x.identifier.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let mut path = Vec::new();
        if let Some(ref x) = value.scoped_identifier_opt {
            path.push(x.dollar.dollar_token.token.text);
        }
        path.push(value.identifier.identifier_token.token.text);
        for x in &value.scoped_identifier_list {
            path.push(x.identifier.identifier_token.token.text);
        }
        SymbolPath(path)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPath {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let mut path = Vec::new();
        if let Some(ref x) = value.expression_identifier_opt {
            path.push(x.dollar.dollar_token.token.text);
        }
        path.push(value.identifier.identifier_token.token.text);
        match &*value.expression_identifier_group {
            syntax_tree::ExpressionIdentifierGroup::ExpressionIdentifierScoped(x) => {
                let x = &x.expression_identifier_scoped;
                path.push(x.identifier.identifier_token.token.text);
                for x in &x.expression_identifier_scoped_list {
                    path.push(x.identifier.identifier_token.token.text);
                }
            }
            syntax_tree::ExpressionIdentifierGroup::ExpressionIdentifierMember(x) => {
                let x = &x.expression_identifier_member;
                for x in &x.expression_identifier_member_list0 {
                    path.push(x.identifier.identifier_token.token.text);
                }
            }
        }
        SymbolPath(path)
    }
}

#[derive(Clone, Default, Debug)]
pub struct SymbolPathNamespace(pub SymbolPath, pub Namespace);

impl From<&Token> for SymbolPathNamespace {
    fn from(value: &Token) -> Self {
        let namespace = namespace_table::get(value.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&SymbolPathNamespace> for SymbolPathNamespace {
    fn from(value: &SymbolPathNamespace) -> Self {
        value.clone()
    }
}

impl From<(&SymbolPath, &Namespace)> for SymbolPathNamespace {
    fn from(value: (&SymbolPath, &Namespace)) -> Self {
        SymbolPathNamespace(value.0.clone(), value.1.clone())
    }
}

impl From<&syntax_tree::Identifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::Identifier) -> Self {
        let namespace = namespace_table::get(value.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&[syntax_tree::Identifier]> for SymbolPathNamespace {
    fn from(value: &[syntax_tree::Identifier]) -> Self {
        let namespace = namespace_table::get(value[0].identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::HierarchicalIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::HierarchicalIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ScopedIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ScopedIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

impl From<&syntax_tree::ExpressionIdentifier> for SymbolPathNamespace {
    fn from(value: &syntax_tree::ExpressionIdentifier) -> Self {
        let namespace = namespace_table::get(value.identifier.identifier_token.token.id).unwrap();
        SymbolPathNamespace(value.into(), namespace)
    }
}

#[derive(Clone, Debug)]
pub struct ResolveResult {
    pub found: ResolveSymbol,
    pub full_path: Vec<SymbolId>,
}

#[derive(Clone, Debug)]
pub enum ResolveSymbol {
    Symbol(Symbol),
    External,
}

#[derive(Clone, Debug)]
pub struct ResolveError {
    pub last_found: Option<Symbol>,
    pub cause: ResolveErrorCause,
}

#[derive(Clone, Debug)]
pub enum ResolveErrorCause {
    NotFound(StrId),
    Private,
}

impl ResolveError {
    pub fn new(last_found: Option<&Symbol>, cause: ResolveErrorCause) -> Self {
        Self {
            last_found: last_found.cloned(),
            cause,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Assign {
    full_path: Vec<SymbolId>,
    position: Vec<Token>,
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    name_table: HashMap<StrId, Vec<SymbolId>>,
    symbol_table: HashMap<SymbolId, Symbol>,
    project_local_table: HashMap<StrId, HashMap<StrId, StrId>>,
    assign_list: Vec<Assign>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut ret = Self::default();

        // add builtin symbols to $ namespace
        let dollar = resource_table::insert_str("$");
        let mut namespace = Namespace::new();
        namespace.push(dollar);

        for func in DEFINED_NAMESPACES {
            let token = Token::new(func, 0, 0, 0, 0, TokenSource::Builtin);
            let symbol = Symbol::new(
                &token,
                SymbolKind::Namespace,
                &namespace,
                false,
                DocComment::default(),
            );
            let _ = ret.insert(&token, symbol);
        }

        for func in DEFINED_SYSTEM_FUNCTIONS {
            let token = Token::new(func, 0, 0, 0, 0, TokenSource::Builtin);
            let symbol = Symbol::new(
                &token,
                SymbolKind::SystemFunction,
                &namespace,
                false,
                DocComment::default(),
            );
            let _ = ret.insert(&token, symbol);
        }

        ret
    }

    pub fn insert(&mut self, token: &Token, symbol: Symbol) -> Option<SymbolId> {
        let entry = self.name_table.entry(token.text).or_default();
        for id in entry.iter() {
            let item = self.symbol_table.get(id).unwrap();
            if symbol.namespace == item.namespace {
                return None;
            }
        }
        let id = symbol.id;
        entry.push(id);
        self.symbol_table.insert(id, symbol);
        Some(id)
    }

    pub fn get(&self, id: SymbolId) -> Option<Symbol> {
        self.symbol_table.get(&id).cloned()
    }

    pub fn resolve(
        &self,
        path: &SymbolPath,
        namespace: &Namespace,
    ) -> Result<ResolveResult, ResolveError> {
        let mut ret = None;
        let mut last_found = None;
        let mut full_path = Vec::new();
        let mut namespace = namespace.clone();
        let mut inner = false;
        let mut other_prj = false;

        let mut path = path.clone();

        // replace project local name
        let prj = namespace.paths[0];
        let path_head = path.0[0];
        if let Some(map) = self.project_local_table.get(&prj) {
            if let Some(id) = map.get(&path_head) {
                path.0[0] = *id;
            }
        }

        for name in path.as_slice() {
            let mut max_depth = 0;
            ret = None;
            if let Some(ids) = self.name_table.get(name) {
                for id in ids {
                    let symbol = self.symbol_table.get(id).unwrap();
                    let included = if inner {
                        namespace.matched(&symbol.namespace)
                    } else {
                        namespace.included(&symbol.namespace)
                            || symbol.imported.iter().any(|x| namespace.included(x))
                    };
                    if included && symbol.namespace.depth() >= max_depth {
                        symbol.evaluate();
                        ret = Some(symbol);
                        last_found = Some(symbol);
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(ret) = ret {
                    full_path.push(ret.id);
                    match &ret.kind {
                        SymbolKind::Variable(x) => {
                            if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                                let path = SymbolPath::new(x);
                                if let Ok(symbol) = self.resolve(&path, &namespace) {
                                    if let ResolveSymbol::Symbol(symbol) = symbol.found {
                                        namespace = Namespace::new();
                                        for path in &symbol.namespace.paths {
                                            namespace.push(*path);
                                        }
                                        namespace.push(symbol.token.text);
                                        inner = true;
                                    } else {
                                        unreachable!();
                                    }
                                } else {
                                    return Ok(ResolveResult {
                                        found: ResolveSymbol::External,
                                        full_path,
                                    });
                                }
                            }
                        }
                        SymbolKind::StructMember(x) => {
                            if let TypeKind::UserDefined(ref x) = x.r#type.kind {
                                let path = SymbolPath::new(x);
                                if let Ok(symbol) = self.resolve(&path, &namespace) {
                                    if let ResolveSymbol::Symbol(symbol) = symbol.found {
                                        namespace = Namespace::new();
                                        for path in &symbol.namespace.paths {
                                            namespace.push(*path);
                                        }
                                        namespace.push(symbol.token.text);
                                        inner = true;
                                    } else {
                                        unreachable!();
                                    }
                                } else {
                                    return Ok(ResolveResult {
                                        found: ResolveSymbol::External,
                                        full_path,
                                    });
                                }
                            }
                        }
                        SymbolKind::Module(_) => {
                            if other_prj & !ret.public {
                                return Err(ResolveError::new(
                                    last_found,
                                    ResolveErrorCause::Private,
                                ));
                            }
                        }
                        SymbolKind::Interface(_) => {
                            if other_prj & !ret.public {
                                return Err(ResolveError::new(
                                    last_found,
                                    ResolveErrorCause::Private,
                                ));
                            }
                            namespace = Namespace::default();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Package(_) => {
                            if other_prj & !ret.public {
                                return Err(ResolveError::new(
                                    last_found,
                                    ResolveErrorCause::Private,
                                ));
                            }
                            namespace = Namespace::default();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Enum(_) => {
                            namespace = ret.namespace.clone();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Instance(ref x) => {
                            namespace = Namespace::default();
                            for x in &x.type_name {
                                namespace.push(*x);
                            }
                            inner = true;
                        }
                        SymbolKind::Port(ref x) if x.direction == Direction::Modport => {
                            if let Some(ref x) = x.r#type {
                                if let TypeKind::UserDefined(ref x) = x.kind {
                                    namespace = Namespace::default();
                                    for x in x {
                                        namespace.push(*x);
                                    }
                                    inner = true;
                                }
                            }
                        }
                        SymbolKind::SystemVerilog => {
                            namespace = ret.namespace.clone();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        SymbolKind::Namespace => {
                            namespace = ret.namespace.clone();
                            namespace.push(ret.token.text);
                            inner = true;
                        }
                        _ => (),
                    }
                } else {
                    return Err(ResolveError::new(
                        last_found,
                        ResolveErrorCause::NotFound(*name),
                    ));
                }
            } else {
                // If symbol is not found, the name is treated as namespace
                namespace = Namespace::new();
                namespace.push(*name);
                inner = true;
                other_prj = true;
            }
        }
        if let Some(ret) = ret {
            Ok(ResolveResult {
                found: ResolveSymbol::Symbol(ret.clone()),
                full_path,
            })
        } else if format!("{}", path.as_slice()[0]) == "$" {
            let cause = ResolveErrorCause::NotFound(path.as_slice()[1]);
            Err(ResolveError::new(last_found, cause))
        } else {
            let cause = ResolveErrorCause::NotFound(path.as_slice()[0]);
            Err(ResolveError::new(last_found, cause))
        }
    }

    pub fn get_all(&self) -> Vec<Symbol> {
        let mut ret = Vec::new();
        for symbol in self.symbol_table.values() {
            symbol.evaluate();
            ret.push(symbol.clone());
        }
        ret
    }

    pub fn dump(&self) -> String {
        for symbol in self.symbol_table.values() {
            symbol.evaluate();
        }
        format!("{self}")
    }

    pub fn dump_assign_list(&self) -> String {
        let mut ret = "AssignList [\n".to_string();

        let mut path_width = 0;
        let mut pos_width = 0;
        for assign in &self.assign_list {
            path_width = path_width.max(
                assign
                    .full_path
                    .iter()
                    .map(|x| self.symbol_table.get(x).unwrap())
                    .map(|x| format!("{}", x.token.text).len())
                    .sum::<usize>()
                    + assign.full_path.len()
                    - 1,
            );
            pos_width = pos_width.max(
                assign
                    .position
                    .iter()
                    .map(|x| x.to_string().len())
                    .sum::<usize>()
                    + assign.position.len()
                    - 1,
            );
        }

        for assign in &self.assign_list {
            let mut path = "".to_string();
            for (i, x) in assign.full_path.iter().enumerate() {
                let x = self.symbol_table.get(x).unwrap();
                if i == 0 {
                    path.push_str(&x.token.to_string());
                } else {
                    path.push_str(&format!(".{}", x.token));
                }
            }

            let mut pos = "".to_string();
            for (i, x) in assign.position.iter().enumerate() {
                if i == 0 {
                    pos.push_str(&x.to_string());
                } else {
                    pos.push_str(&format!(".{}", x));
                }
            }

            let last_token = assign.position.last().unwrap();

            ret.push_str(&format!(
                "    {:path_width$} / {:pos_width$} @ {}:{}:{}\n",
                path,
                pos,
                last_token.source,
                last_token.line,
                last_token.column,
                path_width = path_width,
                pos_width = pos_width,
            ));
        }
        ret.push(']');
        ret
    }

    pub fn drop(&mut self, file_path: PathId) {
        let drop_list: Vec<_> = self
            .symbol_table
            .iter()
            .filter(|x| x.1.token.source == file_path)
            .map(|x| *x.0)
            .collect();

        for id in &drop_list {
            self.symbol_table.remove(id);
        }

        for (_, symbols) in self.name_table.iter_mut() {
            symbols.retain(|x| !drop_list.contains(x));
        }

        for (_, symbol) in self.symbol_table.iter_mut() {
            symbol.references.retain(|x| x.source != file_path);
        }
    }

    pub fn add_reference(&mut self, target: SymbolId, token: &Token) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.id == target {
                symbol.references.push(token.to_owned());
            }
        }
    }

    pub fn add_imported_item(&mut self, target: TokenId, namespace: &Namespace) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.token.id == target {
                symbol.imported.push(namespace.to_owned());
            }
        }
    }

    pub fn add_imported_package(&mut self, target: &Namespace, namespace: &Namespace) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.namespace.matched(target) {
                symbol.imported.push(namespace.to_owned());
            }
        }
    }

    pub fn add_project_local(&mut self, prj: StrId, from: StrId, to: StrId) {
        self.project_local_table
            .entry(prj)
            .and_modify(|x| {
                x.insert(from, to);
            })
            .or_insert(HashMap::from([(from, to)]));
    }

    pub fn get_project_local(&self, prj: StrId) -> Option<HashMap<StrId, StrId>> {
        self.project_local_table.get(&prj).cloned()
    }

    pub fn add_assign(&mut self, full_path: Vec<SymbolId>, position: Vec<Token>) {
        let assign = Assign {
            full_path,
            position,
        };
        self.assign_list.push(assign);
    }

    pub fn get_assign_list(&self) -> Vec<Assign> {
        self.assign_list.clone()
    }
}

impl fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "SymbolTable [")?;
        let mut symbol_width = 0;
        let mut namespace_width = 0;
        let mut reference_width = 0;
        let mut import_width = 0;
        let mut vec: Vec<_> = self.name_table.iter().collect();
        vec.sort_by(|x, y| format!("{}", x.0).cmp(&format!("{}", y.0)));
        for (k, v) in &vec {
            symbol_width = symbol_width.max(format!("{k}").len());
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                namespace_width = namespace_width.max(format!("{}", symbol.namespace).len());
                reference_width = reference_width.max(format!("{}", symbol.references.len()).len());
                import_width = import_width.max(format!("{}", symbol.imported.len()).len());
            }
        }
        for (k, v) in &vec {
            for id in *v {
                let symbol = self.symbol_table.get(id).unwrap();
                let evaluated = if let Some(evaluated) = symbol.evaluated.get() {
                    match evaluated {
                        Evaluated::Unknown => "".to_string(),
                        _ => format!(" ( {evaluated:?} )"),
                    }
                } else {
                    "".to_string()
                };
                writeln!(
                    f,
                    "    {:symbol_width$} @ {:namespace_width$} {{ref: {:reference_width$}, import: {:import_width$}}}: {}{},",
                    k,
                    symbol.namespace,
                    symbol.references.len(),
                    symbol.imported.len(),
                    symbol.kind,
                    evaluated,
                    symbol_width = symbol_width,
                    namespace_width = namespace_width,
                    reference_width = reference_width,
                    import_width = import_width,
                )?;
            }
        }
        writeln!(f, "]")?;

        Ok(())
    }
}

const DEFINED_NAMESPACES: [&str; 1] = ["sv"];

// Refer IEEE Std 1800-2012  Clause 20 and 21
const DEFINED_SYSTEM_FUNCTIONS: [&str; 196] = [
    "acos",
    "acosh",
    "asin",
    "asinh",
    "assertcontrol",
    "assertfailoff",
    "assertfailon",
    "assertkill",
    "assertnonvacuouson",
    "assertoff",
    "asserton",
    "assertpassoff",
    "assertpasson",
    "assertvacuousoff",
    "async$and$array",
    "async$and$plane",
    "async$nand$array",
    "async$nand$plane",
    "async$nor$array",
    "async$nor$plane",
    "async$or$array",
    "async$or$plane",
    "atan",
    "atan2",
    "atanh",
    "bits",
    "bitstoreal",
    "bitstoshortreal",
    "cast",
    "ceil",
    "changed",
    "changed_gclk",
    "changing_gclk",
    "clog2",
    "cos",
    "cosh",
    "countbits",
    "countones",
    "coverage_control",
    "coverage_get",
    "coverage_get_max",
    "coverage_merge",
    "coverage_save",
    "dimensions",
    "display",
    "displayb",
    "displayh",
    "displayo",
    "dist_chi_square",
    "dist_erlang",
    "dist_exponential",
    "dist_normal",
    "dist_poisson",
    "dist_t",
    "dist_uniform",
    "dumpall",
    "dumpfile",
    "dumpflush",
    "dumplimit",
    "dumpoff",
    "dumpon",
    "dumpports",
    "dumpportsall",
    "dumpportsflush",
    "dumpportslimit",
    "dumpportsoff",
    "dumpportson",
    "dumpvars",
    "error",
    "exit",
    "exp",
    "falling_gclk",
    "fatal",
    "fclose",
    "fdisplay",
    "fdisplayb",
    "fdisplayh",
    "fdisplayo",
    "fell",
    "fell_gclk",
    "feof",
    "ferror",
    "fflush",
    "fgetc",
    "fgets",
    "finish",
    "floor",
    "fmonitor",
    "fmonitorb",
    "fmonitorh",
    "fmonitoro",
    "fopen",
    "fread",
    "fscanf",
    "fseek",
    "fstrobe",
    "fstrobeb",
    "fstrobeh",
    "fstrobeo",
    "ftell",
    "future_gclk",
    "fwrite",
    "fwriteb",
    "fwriteh",
    "fwriteo",
    "get_coverage",
    "high",
    "hypot",
    "increment",
    "info",
    "isunbounded",
    "isunknown",
    "itor",
    "left",
    "ln",
    "load_coverage_db",
    "log10",
    "low",
    "monitor",
    "monitorb",
    "monitorh",
    "monitoro",
    "monitoroff",
    "monitoron",
    "onehot",
    "onehot0",
    "past",
    "past_gclk",
    "pow",
    "printtimescale",
    "q_add",
    "q_exam",
    "q_full",
    "q_initialize",
    "q_remove",
    "random",
    "readmemb",
    "readmemh",
    "realtime",
    "realtobits",
    "rewind",
    "right",
    "rising_gclk",
    "rose",
    "rose_gclk",
    "rtoi",
    "sampled",
    "set_coverage_db_name",
    "sformat",
    "sformatf",
    "shortrealtobits",
    "signed",
    "sin",
    "sinh",
    "size",
    "sqrt",
    "sscanf",
    "stable",
    "stable_gclk",
    "steady_gclk",
    "stime",
    "stop",
    "strobe",
    "strobeb",
    "strobeh",
    "strobeo",
    "swrite",
    "swriteb",
    "swriteh",
    "swriteo",
    "sync$and$array",
    "sync$and$plane",
    "sync$nand$array",
    "sync$nand$plane",
    "sync$nor$array",
    "sync$nor$plane",
    "sync$or$array",
    "sync$or$plane",
    "system",
    "tan",
    "tanh",
    "test$plusargs",
    "time",
    "timeformat",
    "typename",
    "ungetc",
    "unpacked_dimensions",
    "unsigned",
    "value$plusargs",
    "warning",
    "write",
    "writeb",
    "writeh",
    "writememb",
    "writememh",
    "writeo",
];

thread_local!(static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::new()));

pub fn insert(token: &Token, symbol: Symbol) -> Option<SymbolId> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().insert(token, symbol))
}

pub fn get(id: SymbolId) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(id))
}

pub fn resolve<T: Into<SymbolPathNamespace>>(path: T) -> Result<ResolveResult, ResolveError> {
    let SymbolPathNamespace(path, namespace) = path.into();
    SYMBOL_TABLE.with(|f| f.borrow().resolve(&path, &namespace))
}

pub fn get_all() -> Vec<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get_all())
}

pub fn dump() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump())
}

pub fn dump_assign_list() -> String {
    SYMBOL_TABLE.with(|f| f.borrow().dump_assign_list())
}

pub fn drop(file_path: PathId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().drop(file_path))
}

pub fn add_reference(target: SymbolId, token: &Token) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_reference(target, token))
}

pub fn add_imported_item(target: TokenId, namespace: &Namespace) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_imported_item(target, namespace))
}

pub fn add_imported_package(target: &Namespace, namespace: &Namespace) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_imported_package(target, namespace))
}

pub fn add_project_local(prj: StrId, from: StrId, to: StrId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_project_local(prj, from, to))
}

pub fn get_project_local(prj: StrId) -> Option<HashMap<StrId, StrId>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_project_local(prj))
}

pub fn add_assign(full_path: Vec<SymbolId>, position: Vec<Token>) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_assign(full_path, position))
}

pub fn get_assign_list() -> Vec<Assign> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().get_assign_list())
}

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::{ResolveSymbol, SymbolPath};
    use crate::{symbol_table, Analyzer};
    use veryl_metadata::Metadata;
    use veryl_parser::{resource_table, Parser};

    const CODE: &str = r##"
    module ModuleA #(
        param paramA: u32 = 1,
    ) (
        portA: input logic<10>,
    ) {
        local paramB: u32 = 1;

        var memberA: logic;
        var memberB: PackageA::StructA;
    }

    interface InterfaceA #(
        param paramA: u32 = 1,
    ) {
        local paramB: u32 = 1;

        var memberA: logic;

        modport modportA {
            memberA: input,
        }
    }

    package PackageA {
        local paramB: u32 = 1;

        struct StructX {
            memberY: logic,
        }

        struct StructA {
            memberA: logic,
            memberX: StructX,
        }

        enum EnumA: logic<2> {
            memberA,
        }
    }
    "##;

    fn parse() {
        let metadata: Metadata =
            toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();
        let parser = Parser::parse(&CODE, &"").unwrap();
        let analyzer = Analyzer::new(&metadata);
        analyzer.analyze_pass1(&"prj", &CODE, &"", &parser.veryl);
    }

    fn check_namespace(symbol: ResolveSymbol, expect: &str) {
        if let ResolveSymbol::Symbol(symbol) = symbol {
            assert_eq!(format!("{}", symbol.namespace), expect);
        } else {
            assert!(false);
        }
    }

    #[test]
    fn module() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");
    }

    #[test]
    fn interface() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");
    }

    #[test]
    fn package() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("PackageA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj");
    }

    #[test]
    fn parameter() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::InterfaceA");
    }

    #[test]
    fn localparam() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("paramB".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::InterfaceA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::PackageA");
    }

    #[test]
    fn port() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("portA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());
    }

    #[test]
    fn variable() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::ModuleA");

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::InterfaceA");
    }

    #[test]
    fn r#struct() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("StructA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("InterfaceA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let namespace = Namespace::default();
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("PackageA".to_string()).unwrap());
        namespace.push(resource_table::get_str_id("StructA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::PackageA::StructA");

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberA".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::PackageA::StructA");

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_err());
    }

    #[test]
    fn nest_struct() {
        parse();

        let mut symbol_path = SymbolPath::default();
        symbol_path.push(resource_table::get_str_id("memberB".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberX".to_string()).unwrap());
        symbol_path.push(resource_table::get_str_id("memberY".to_string()).unwrap());

        let mut namespace = Namespace::default();
        namespace.push(resource_table::get_str_id("ModuleA".to_string()).unwrap());
        let symbol = symbol_table::resolve((&symbol_path, &namespace));

        assert!(symbol.is_ok());
        check_namespace(symbol.unwrap().found, "prj::PackageA::StructX");
    }
}
