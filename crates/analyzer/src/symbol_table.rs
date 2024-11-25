use crate::evaluator::Evaluated;
use crate::namespace::Namespace;
use crate::symbol::{DocComment, Symbol, SymbolId, SymbolKind, TypeKind};
use crate::symbol_path::{SymbolPath, SymbolPathNamespace};
use crate::var_ref::{Assign, VarRef, VarRefAffiliation};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use veryl_parser::resource_table::{PathId, StrId, TokenId};
use veryl_parser::veryl_token::{Token, TokenSource};

#[derive(Clone, Debug)]
pub struct ResolveResult {
    pub found: Symbol,
    pub full_path: Vec<SymbolId>,
    pub imported: bool,
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
pub struct Import {
    pub path: SymbolPathNamespace,
    pub namespace: Namespace,
    pub wildcard: bool,
}

#[derive(Clone, Default, Debug)]
pub struct SymbolTable {
    name_table: HashMap<StrId, Vec<SymbolId>>,
    symbol_table: HashMap<SymbolId, Symbol>,
    project_local_table: HashMap<StrId, HashMap<StrId, StrId>>,
    var_ref_list: HashMap<VarRefAffiliation, Vec<VarRef>>,
    import_list: Vec<Import>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut ret = Self::default();

        // add builtin symbols to "" namespace
        let namespace = Namespace::new();

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

    pub fn update(&mut self, symbol: Symbol) {
        let id = symbol.id;
        self.symbol_table.insert(id, symbol);
    }

    fn trace_user_defined<'a>(
        &self,
        mut context: ResolveContext<'a>,
        kind: &TypeKind,
    ) -> Result<ResolveContext<'a>, ResolveError> {
        if let TypeKind::UserDefined(ref x) = kind {
            // Detect infinite loop in trace_user_defined
            if let Some(last_found) = context.last_found {
                if *x.first().unwrap() == last_found.token.text {
                    return Ok(context);
                }
            }

            let symbol = self.resolve(&SymbolPath::new(x), &context.namespace)?;
            match symbol.found.kind {
                SymbolKind::SystemVerilog => context.sv_member = true,
                SymbolKind::TypeDef(x) => {
                    return self.trace_user_defined(context, &x.r#type.kind);
                }
                _ => (),
            }
            context.namespace = symbol.found.inner_namespace();
            context.inner = true;
        }
        Ok(context)
    }

    pub fn resolve(
        &self,
        path: &SymbolPath,
        namespace: &Namespace,
    ) -> Result<ResolveResult, ResolveError> {
        let mut context = ResolveContext::new(namespace);
        let mut path = path.clone();

        // replace project local name
        let prj = context.namespace.paths[0];
        let path_head = path.0[0];
        if let Some(map) = self.project_local_table.get(&prj) {
            if let Some(id) = map.get(&path_head) {
                path.0[0] = *id;
            }
        }

        for name in path.as_slice() {
            let mut max_depth = 0;
            context.found = None;

            if context.sv_member {
                let token = Token::new(&name.to_string(), 0, 0, 0, 0, TokenSource::External);
                let symbol = Symbol::new(
                    &token,
                    SymbolKind::SystemVerilog,
                    &context.namespace,
                    false,
                    DocComment::default(),
                );
                return Ok(ResolveResult {
                    found: symbol,
                    full_path: context.full_path,
                    imported: context.imported,
                });
            }

            if let Some(ids) = self.name_table.get(name) {
                for id in ids {
                    let symbol = self.symbol_table.get(id).unwrap();
                    let (included, imported) = if context.inner {
                        (context.namespace.matched(&symbol.namespace), false)
                    } else {
                        let imported = symbol
                            .imported
                            .iter()
                            .any(|x| context.namespace.included(x));
                        (
                            context.namespace.included(&symbol.namespace) || imported,
                            imported,
                        )
                    };
                    if included && symbol.namespace.depth() >= max_depth {
                        symbol.evaluate();
                        context.found = Some(symbol);
                        context.last_found = Some(symbol);
                        context.imported = imported;
                        max_depth = symbol.namespace.depth();
                    }
                }

                if let Some(found) = context.found {
                    context.full_path.push(found.id);
                    match &found.kind {
                        SymbolKind::Variable(x) => {
                            context = self.trace_user_defined(context, &x.r#type.kind)?;
                        }
                        SymbolKind::StructMember(x) => {
                            context = self.trace_user_defined(context, &x.r#type.kind)?;
                        }
                        SymbolKind::UnionMember(x) => {
                            context = self.trace_user_defined(context, &x.r#type.kind)?;
                        }
                        SymbolKind::Parameter(x) => {
                            context = self.trace_user_defined(context, &x.r#type.kind)?;
                        }
                        SymbolKind::TypeDef(x) => {
                            context = self.trace_user_defined(context, &x.r#type.kind)?;
                        }
                        SymbolKind::Port(ref x) => {
                            if let Some(ref x) = x.r#type {
                                context = self.trace_user_defined(context, &x.kind)?;
                            }
                        }
                        SymbolKind::ModportVariableMember(_) => {
                            let path = SymbolPath::new(&[found.token.text]);
                            context.namespace = found.namespace.clone();
                            context.namespace.pop();
                            let symbol = self.resolve(&path, &context.namespace)?;
                            if let SymbolKind::Variable(x) = &symbol.found.kind {
                                context = self.trace_user_defined(context, &x.r#type.kind)?;
                            }
                        }
                        SymbolKind::Module(_)
                        | SymbolKind::Interface(_)
                        | SymbolKind::Package(_) => {
                            if context.other_prj & !found.public {
                                return Err(ResolveError::new(
                                    context.last_found,
                                    ResolveErrorCause::Private,
                                ));
                            }
                            context.namespace = found.inner_namespace();
                            context.inner = true;
                        }
                        SymbolKind::Enum(_) | SymbolKind::SystemVerilog | SymbolKind::Namespace => {
                            context.namespace = found.inner_namespace();
                            context.inner = true;
                        }
                        SymbolKind::Instance(ref x) => {
                            let mut type_name = x.type_name.clone();
                            type_name.resolve_imported(&context.namespace);
                            let path = type_name.mangled_path();
                            let symbol = self.resolve(&path, &context.namespace)?;
                            if let SymbolKind::GenericInstance(x) = &symbol.found.kind {
                                let symbol = self.symbol_table.get(&x.base).unwrap();
                                context.namespace = symbol.inner_namespace();
                                context.inner = true;
                            } else {
                                context.namespace = Namespace::default();
                                for x in path.as_slice() {
                                    context.namespace.push(*x);
                                }
                                context.inner = true;
                            }
                        }
                        SymbolKind::GenericInstance(ref x) => {
                            let symbol = self.symbol_table.get(&x.base).unwrap();
                            context.namespace = symbol.inner_namespace();
                            context.inner = true;
                            context
                                .generic_namespace_map
                                .insert(symbol.token.text, found.token.text);
                        }
                        SymbolKind::GenericParameter(_) => {
                            context.namespace = found.inner_namespace();
                            context.inner = true;
                        }
                        // don't trace inner item
                        SymbolKind::Function(_)
                        | SymbolKind::ProtoModule(_)
                        | SymbolKind::Struct(_)
                        | SymbolKind::Union(_)
                        | SymbolKind::Modport(_)
                        | SymbolKind::ModportFunctionMember(_)
                        | SymbolKind::EnumMember(_)
                        | SymbolKind::EnumMemberMangled
                        | SymbolKind::Block
                        | SymbolKind::SystemFunction
                        | SymbolKind::Genvar
                        | SymbolKind::ClockDomain
                        | SymbolKind::Test(_) => (),
                    }
                } else {
                    return Err(ResolveError::new(
                        context.last_found,
                        ResolveErrorCause::NotFound(*name),
                    ));
                }
            } else {
                // If symbol is not found, the name is treated as namespace
                context.namespace = Namespace::new();
                context.namespace.push(*name);
                context.inner = true;
                context.other_prj = true;
            }
        }
        if let Some(found) = context.found {
            let mut found = found.clone();

            // replace namespace path to generic version
            let generic_namespace = found.namespace.replace(&context.generic_namespace_map);
            found.namespace = generic_namespace;

            Ok(ResolveResult {
                found,
                full_path: context.full_path,
                imported: context.imported,
            })
        } else {
            let cause = ResolveErrorCause::NotFound(context.namespace.pop().unwrap());
            Err(ResolveError::new(context.last_found, cause))
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
        let assign_list = self.get_assign_list();

        let mut ret = "AssignList [\n".to_string();

        let mut path_width = 0;
        let mut pos_width = 0;
        for assign in &assign_list {
            path_width = path_width.max(assign.path.to_string().len());
            pos_width = pos_width.max(assign.position.to_string().len());
        }

        for assign in &assign_list {
            let last_token = assign.position.0.last().unwrap().token();

            ret.push_str(&format!(
                "    {:path_width$} / {:pos_width$} @ {}:{}:{}\n",
                assign.path,
                assign.position,
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
                break;
            }
        }
    }

    pub fn add_generic_instance(&mut self, target: SymbolId, instance: SymbolId) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.id == target && !symbol.generic_instances.contains(&instance) {
                symbol.generic_instances.push(instance);
                break;
            }
        }
    }

    fn add_imported_item(&mut self, target: TokenId, namespace: &Namespace) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.token.id == target {
                symbol.imported.push(namespace.to_owned());
            }
        }
    }

    fn add_imported_package(&mut self, target: &Namespace, namespace: &Namespace) {
        for (_, symbol) in self.symbol_table.iter_mut() {
            if symbol.namespace.matched(target) {
                symbol.imported.push(namespace.to_owned());
            }
        }
    }

    pub fn add_import(&mut self, import: Import) {
        self.import_list.push(import);
    }

    pub fn apply_import(&mut self) {
        let import_list: Vec<_> = self.import_list.drain(0..).collect();
        for import in import_list {
            if let Ok(symbol) = self.resolve(&import.path.0, &import.path.1) {
                let symbol = symbol.found;
                match symbol.kind {
                    SymbolKind::Package(_) if import.wildcard => {
                        let mut target = symbol.namespace.clone();
                        target.push(symbol.token.text);

                        self.add_imported_package(&target, &import.namespace);
                    }
                    SymbolKind::SystemVerilog => (),
                    // Error will be reported at create_reference
                    _ if import.wildcard => (),
                    _ => {
                        self.add_imported_item(symbol.token.id, &import.namespace);
                    }
                }
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

    pub fn add_var_ref(&mut self, var_ref: &VarRef) {
        self.var_ref_list
            .entry(var_ref.affiliation)
            .and_modify(|x| x.push(var_ref.clone()))
            .or_insert(vec![var_ref.clone()]);
    }

    pub fn get_var_ref_list(&self) -> HashMap<VarRefAffiliation, Vec<VarRef>> {
        self.var_ref_list.clone()
    }

    pub fn get_assign_list(&self) -> Vec<Assign> {
        self.var_ref_list
            .values()
            .flat_map(|l| l.iter().filter(|r| r.is_assign()))
            .map(Assign::new)
            .collect()
    }

    pub fn clear(&mut self) {
        self.clone_from(&Self::new());
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

struct ResolveContext<'a> {
    found: Option<&'a Symbol>,
    last_found: Option<&'a Symbol>,
    full_path: Vec<SymbolId>,
    namespace: Namespace,
    generic_namespace_map: HashMap<StrId, StrId>,
    inner: bool,
    other_prj: bool,
    sv_member: bool,
    imported: bool,
}

impl<'a> ResolveContext<'a> {
    fn new(namespace: &Namespace) -> Self {
        Self {
            found: None,
            last_found: None,
            full_path: vec![],
            namespace: namespace.clone(),
            generic_namespace_map: HashMap::new(),
            inner: false,
            other_prj: false,
            sv_member: false,
            imported: false,
        }
    }
}

const DEFINED_NAMESPACES: [&str; 2] = ["$sv", "$std"];

// Refer IEEE Std 1800-2023 Table B.1 - Reserved keywords
// This list must be sorted to enable binary search
const SYSTEMVERILOG_KEYWORDS: [&str; 248] = [
    "accept_on",
    "alias",
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "and",
    "assert",
    "assign",
    "assume",
    "automatic",
    "before",
    "begin",
    "bind",
    "bins",
    "binsof",
    "bit",
    "break",
    "buf",
    "bufif0",
    "bufif1",
    "byte",
    "case",
    "casex",
    "casez",
    "cell",
    "chandle",
    "checker",
    "class",
    "clocking",
    "cmos",
    "config",
    "const",
    "constraint",
    "context",
    "continue",
    "cover",
    "covergroup",
    "coverpoint",
    "cross",
    "deassign",
    "default",
    "defparam",
    "design",
    "disable",
    "dist",
    "do",
    "edge",
    "else",
    "end",
    "endcase",
    "endchecker",
    "endclass",
    "endclocking",
    "endconfig",
    "endfunction",
    "endgenerate",
    "endgroup",
    "endinterface",
    "endmodule",
    "endpackage",
    "endprimitive",
    "endprogram",
    "endproperty",
    "endspecify",
    "endsequence",
    "endtable",
    "endtask",
    "enum",
    "event",
    "eventually",
    "expect",
    "export",
    "extends",
    "extern",
    "final",
    "first_match",
    "for",
    "force",
    "foreach",
    "forever",
    "fork",
    "forkjoin",
    "function",
    "generate",
    "genvar",
    "global",
    "highz0",
    "highz1",
    "if",
    "iff",
    "ifnone",
    "ignore_bins",
    "illegal_bins",
    "implements",
    "implies",
    "import",
    "incdir",
    "include",
    "initial",
    "inout",
    "input",
    "inside",
    "instance",
    "int",
    "integer",
    "interconnect",
    "interface",
    "intersect",
    "join",
    "join_any",
    "join_none",
    "large",
    "let",
    "liblist",
    "library",
    "local",
    "localparam",
    "logic",
    "longint",
    "macromodule",
    "matches",
    "medium",
    "modport",
    "module",
    "nand",
    "negedge",
    "nettype",
    "new",
    "nexttime",
    "nmos",
    "nor",
    "noshowcancelled",
    "not",
    "notif0",
    "notif1",
    "null",
    "or",
    "output",
    "package",
    "packed",
    "parameter",
    "pmos",
    "posedge",
    "primitive",
    "priority",
    "program",
    "property",
    "protected",
    "pull0",
    "pull1",
    "pulldown",
    "pullup",
    "pulsestyle_ondetect",
    "pulsestyle_onevent",
    "pure",
    "rand",
    "randc",
    "randcase",
    "randsequence",
    "rcmos",
    "real",
    "realtime",
    "ref",
    "reg",
    "reject_on",
    "release",
    "repeat",
    "restrict",
    "return",
    "rnmos",
    "rpmos",
    "rtran",
    "rtranif0",
    "rtranif1",
    "s_always",
    "s_eventually",
    "s_nexttime",
    "s_until",
    "s_until_with",
    "scalared",
    "sequence",
    "shortint",
    "shortreal",
    "showcancelled",
    "signed",
    "small",
    "soft",
    "solve",
    "specify",
    "specparam",
    "static",
    "string",
    "strong",
    "strong0",
    "strong1",
    "struct",
    "super",
    "supply0",
    "supply1",
    "sync_accept_on",
    "sync_reject_on",
    "table",
    "tagged",
    "task",
    "this",
    "throughout",
    "time",
    "timeprecision",
    "timeunit",
    "tran",
    "tranif0",
    "tranif1",
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "type",
    "typedef",
    "union",
    "unique",
    "unique0",
    "unsigned",
    "until",
    "until_with",
    "untyped",
    "use",
    "uwire",
    "var",
    "vectored",
    "virtual",
    "void",
    "wait",
    "wait_order",
    "wand",
    "weak",
    "weak0",
    "weak1",
    "while",
    "wildcard",
    "wire",
    "with",
    "within",
    "wor",
    "xnor",
    "xor",
];

pub fn is_sv_keyword(s: &str) -> bool {
    SYSTEMVERILOG_KEYWORDS.binary_search(&s).is_ok()
}

// Refer IEEE Std 1800-2012  Clause 20 and 21
const DEFINED_SYSTEM_FUNCTIONS: [&str; 196] = [
    "$acos",
    "$acosh",
    "$asin",
    "$asinh",
    "$assertcontrol",
    "$assertfailoff",
    "$assertfailon",
    "$assertkill",
    "$assertnonvacuouson",
    "$assertoff",
    "$asserton",
    "$assertpassoff",
    "$assertpasson",
    "$assertvacuousoff",
    "$async$and$array",
    "$async$and$plane",
    "$async$nand$array",
    "$async$nand$plane",
    "$async$nor$array",
    "$async$nor$plane",
    "$async$or$array",
    "$async$or$plane",
    "$atan",
    "$atan2",
    "$atanh",
    "$bits",
    "$bitstoreal",
    "$bitstoshortreal",
    "$cast",
    "$ceil",
    "$changed",
    "$changed_gclk",
    "$changing_gclk",
    "$clog2",
    "$cos",
    "$cosh",
    "$countbits",
    "$countones",
    "$coverage_control",
    "$coverage_get",
    "$coverage_get_max",
    "$coverage_merge",
    "$coverage_save",
    "$dimensions",
    "$display",
    "$displayb",
    "$displayh",
    "$displayo",
    "$dist_chi_square",
    "$dist_erlang",
    "$dist_exponential",
    "$dist_normal",
    "$dist_poisson",
    "$dist_t",
    "$dist_uniform",
    "$dumpall",
    "$dumpfile",
    "$dumpflush",
    "$dumplimit",
    "$dumpoff",
    "$dumpon",
    "$dumpports",
    "$dumpportsall",
    "$dumpportsflush",
    "$dumpportslimit",
    "$dumpportsoff",
    "$dumpportson",
    "$dumpvars",
    "$error",
    "$exit",
    "$exp",
    "$falling_gclk",
    "$fatal",
    "$fclose",
    "$fdisplay",
    "$fdisplayb",
    "$fdisplayh",
    "$fdisplayo",
    "$fell",
    "$fell_gclk",
    "$feof",
    "$ferror",
    "$fflush",
    "$fgetc",
    "$fgets",
    "$finish",
    "$floor",
    "$fmonitor",
    "$fmonitorb",
    "$fmonitorh",
    "$fmonitoro",
    "$fopen",
    "$fread",
    "$fscanf",
    "$fseek",
    "$fstrobe",
    "$fstrobeb",
    "$fstrobeh",
    "$fstrobeo",
    "$ftell",
    "$future_gclk",
    "$fwrite",
    "$fwriteb",
    "$fwriteh",
    "$fwriteo",
    "$get_coverage",
    "$high",
    "$hypot",
    "$increment",
    "$info",
    "$isunbounded",
    "$isunknown",
    "$itor",
    "$left",
    "$ln",
    "$load_coverage_db",
    "$log10",
    "$low",
    "$monitor",
    "$monitorb",
    "$monitorh",
    "$monitoro",
    "$monitoroff",
    "$monitoron",
    "$onehot",
    "$onehot0",
    "$past",
    "$past_gclk",
    "$pow",
    "$printtimescale",
    "$q_add",
    "$q_exam",
    "$q_full",
    "$q_initialize",
    "$q_remove",
    "$random",
    "$readmemb",
    "$readmemh",
    "$realtime",
    "$realtobits",
    "$rewind",
    "$right",
    "$rising_gclk",
    "$rose",
    "$rose_gclk",
    "$rtoi",
    "$sampled",
    "$set_coverage_db_name",
    "$sformat",
    "$sformatf",
    "$shortrealtobits",
    "$signed",
    "$sin",
    "$sinh",
    "$size",
    "$sqrt",
    "$sscanf",
    "$stable",
    "$stable_gclk",
    "$steady_gclk",
    "$stime",
    "$stop",
    "$strobe",
    "$strobeb",
    "$strobeh",
    "$strobeo",
    "$swrite",
    "$swriteb",
    "$swriteh",
    "$swriteo",
    "$sync$and$array",
    "$sync$and$plane",
    "$sync$nand$array",
    "$sync$nand$plane",
    "$sync$nor$array",
    "$sync$nor$plane",
    "$sync$or$array",
    "$sync$or$plane",
    "$system",
    "$tan",
    "$tanh",
    "$test$plusargs",
    "$time",
    "$timeformat",
    "$typename",
    "$ungetc",
    "$unpacked_dimensions",
    "$unsigned",
    "$value$plusargs",
    "$warning",
    "$write",
    "$writeb",
    "$writeh",
    "$writememb",
    "$writememh",
    "$writeo",
];

thread_local!(static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::new()));

pub fn insert(token: &Token, symbol: Symbol) -> Option<SymbolId> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().insert(token, symbol))
}

pub fn get(id: SymbolId) -> Option<Symbol> {
    SYMBOL_TABLE.with(|f| f.borrow().get(id))
}

pub fn update(symbol: Symbol) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().update(symbol))
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

pub fn add_generic_instance(target: SymbolId, instance: SymbolId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_generic_instance(target, instance))
}

pub fn add_import(import: Import) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_import(import))
}

pub fn apply_import() {
    SYMBOL_TABLE.with(|f| f.borrow_mut().apply_import())
}

pub fn add_project_local(prj: StrId, from: StrId, to: StrId) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_project_local(prj, from, to))
}

pub fn get_project_local(prj: StrId) -> Option<HashMap<StrId, StrId>> {
    SYMBOL_TABLE.with(|f| f.borrow().get_project_local(prj))
}

pub fn add_var_ref(var_ref: &VarRef) {
    SYMBOL_TABLE.with(|f| f.borrow_mut().add_var_ref(var_ref))
}

pub fn get_var_ref_list() -> HashMap<VarRefAffiliation, Vec<VarRef>> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().get_var_ref_list())
}

pub fn get_assign_list() -> Vec<Assign> {
    SYMBOL_TABLE.with(|f| f.borrow_mut().get_assign_list())
}

pub fn clear() {
    SYMBOL_TABLE.with(|f| f.borrow_mut().clear())
}

#[cfg(test)]
mod tests {
    use crate::namespace::Namespace;
    use crate::symbol_table::{ResolveError, ResolveResult, SymbolPath};
    use crate::{symbol_table, Analyzer};
    use veryl_metadata::Metadata;
    use veryl_parser::{resource_table, Parser};

    const CODE: &str = r##"
    module ModuleA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) (
        portA: input logic<10>,
        portB: modport InterfaceA::modportA,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;
        var memberD: $sv::SvTypeA;
        var memberE: PackageA::UnionA;

        inst instA: InterfaceA;
    }

    interface InterfaceA #(
        param paramA: u32 = 1,
        param paramB: PackageA::StructA = 1,
    ) {
        const localA: u32 = 1;
        const localB: PackageA::StructA = 1;

        type TypeA = PackageA::StructA;

        var memberA: logic;
        var memberB: PackageA::StructA;
        var memberC: TypeA;

        modport modportA {
            memberA: input,
            memberB: output,
            memberC: output,
        }
    }

    package PackageA {
        const localA: u32 = 1;

        struct StructA {
            memberA: logic,
            memberB: StructB,
        }

        struct StructB {
            memberA: logic,
        }

        enum EnumA: logic<2> {
            memberA,
        }

        union UnionA {
            memberA: logic<2>,
            memberB: EnumA,
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

    #[track_caller]
    fn check_found(result: Result<ResolveResult, ResolveError>, expect: &str) {
        assert_eq!(format!("{}", result.unwrap().found.namespace), expect);
    }

    #[track_caller]
    fn check_not_found(result: Result<ResolveResult, ResolveError>) {
        assert!(result.is_err());
    }

    fn create_path(paths: &[&str]) -> SymbolPath {
        let mut ret = SymbolPath::default();

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn create_namespace(paths: &[&str]) -> Namespace {
        let mut ret = Namespace::default();

        for path in paths {
            ret.push(resource_table::insert_str(path));
        }

        ret
    }

    fn resolve(paths: &[&str], namespace: &[&str]) -> Result<ResolveResult, ResolveError> {
        let path = create_path(paths);
        let namespace = create_namespace(namespace);
        symbol_table::resolve((&path, &namespace))
    }

    #[test]
    fn module() {
        parse();

        let symbol = resolve(&["ModuleA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["ModuleA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn interface() {
        parse();

        let symbol = resolve(&["InterfaceA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn package() {
        parse();

        let symbol = resolve(&["PackageA"], &[]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["ModuleA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["InterfaceA"]);
        check_found(symbol, "prj");

        let symbol = resolve(&["InterfaceA"], &["PackageA"]);
        check_found(symbol, "prj");
    }

    #[test]
    fn param() {
        parse();

        let symbol = resolve(&["paramA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["paramA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["paramA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["paramA"], &["PackageA"]);
        check_not_found(symbol);

        let symbol = resolve(&["paramB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["paramB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn local() {
        parse();

        let symbol = resolve(&["localA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["localA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["localA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["localA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");

        let symbol = resolve(&["localB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["localB", "memberB", "memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn port() {
        parse();

        let symbol = resolve(&["portA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["portA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn variable() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberA"], &["InterfaceA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["memberA"], &["PackageA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#struct() {
        parse();

        let symbol = resolve(&["StructA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["StructA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn struct_member() {
        parse();

        let symbol = resolve(&["memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberA"], &["PackageA", "StructA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberB", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn r#enum() {
        parse();

        let symbol = resolve(&["EnumA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn enum_member() {
        parse();

        let symbol = resolve(&["EnumA", "memberA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["EnumA", "memberA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA::EnumA");
    }

    #[test]
    fn union() {
        parse();

        let symbol = resolve(&["UnionA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["ModuleA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["InterfaceA"]);
        check_not_found(symbol);

        let symbol = resolve(&["UnionA"], &["PackageA"]);
        check_found(symbol, "prj::PackageA");
    }

    #[test]
    fn union_member() {
        parse();

        let symbol = resolve(&["memberE"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberE"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberE", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");

        let symbol = resolve(&["memberE", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::UnionA");
    }

    #[test]
    fn modport() {
        parse();

        let symbol = resolve(&["portB"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["portB"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["portB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");

        let symbol = resolve(&["portB", "memberC"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA::modportA");

        let symbol = resolve(&["portB", "memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["portB", "memberC", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }

    #[test]
    fn typedef() {
        parse();

        let symbol = resolve(&["memberC"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberC"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberC", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructA");

        let symbol = resolve(&["memberC", "memberX"], &["ModuleA"]);
        check_not_found(symbol);
    }

    #[test]
    fn sv_member() {
        parse();

        let symbol = resolve(&["memberD"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["memberD"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["memberD", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");

        let symbol = resolve(&["memberD", "memberA", "memberA", "memberA"], &["ModuleA"]);
        check_found(symbol, "$sv::SvTypeA");
    }

    #[test]
    fn inst() {
        parse();

        let symbol = resolve(&["instA"], &[]);
        check_not_found(symbol);

        let symbol = resolve(&["instA"], &["ModuleA"]);
        check_found(symbol, "prj::ModuleA");

        let symbol = resolve(&["instA", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::InterfaceA");

        let symbol = resolve(&["instA", "memberB", "memberB", "memberA"], &["ModuleA"]);
        check_found(symbol, "prj::PackageA::StructB");
    }
}
