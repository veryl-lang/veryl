//! Parsing of component interface manifests (the JSON emitted by the
//! `#[component]` attribute), shared by the simulator's load-time checks
//! and the analyzer's analysis-time checks.

use serde_json::Value as Json;
use std::collections::HashMap;

#[derive(Clone)]
pub struct ManifestPort {
    pub name: String,
    pub dir: String,
    /// `"clock"`/`"reset"` for ports declared with a dedicated role;
    /// `None` for plain data ports. Widths are inferred from the connected
    /// interface, so the manifest does not record them.
    pub role: Option<String>,
    pub doc: Option<String>,
}

/// A group bound to a specific interface + modport, carrying its
/// member declarations; a connection whose modport belongs to a different
/// interface is rejected. `interface` is the path as the component-defining
/// project sees it (e.g. `"$std::axi4_if"`), resolved to a symbol at
/// connection time.
#[derive(Clone)]
pub struct ManifestGroup {
    pub name: String,
    pub interface: String,
    pub modport: String,
    pub members: Vec<ManifestMember>,
    pub doc: Option<String>,
}

/// One interface member a group binds; the connected interface must
/// provide it.
#[derive(Clone)]
pub struct ManifestMember {
    pub member: String,
    pub dir: String,
    pub doc: Option<String>,
}

/// What a connection resolves to; see
/// [`ComponentManifest::connection_target`].
pub enum ConnectionTarget<'a> {
    Loose(&'a ManifestPort),
    Member(&'a ManifestGroup, &'a ManifestMember),
}

/// What a connection offers to the declaration it binds.
pub struct ConnectionFacts {
    pub input: bool,
    pub drivable: bool,
    pub is_clock: bool,
    pub is_reset: bool,
}

/// A connection-vs-declaration mismatch, evaluated by
/// [`ConnectionTarget::check`]. The analyzer and the simulator's load-time
/// check map these to their own error types, so the rules cannot drift
/// between the two.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConnectionViolation {
    /// The manifest declares a direction that is neither input nor output
    /// (a corrupt manifest; they are machine-generated).
    InvalidDirection(String),
    NotInput,
    NotDrivable,
    NotClock,
    NotReset,
    ClockUndeclared,
    ResetUndeclared,
}

impl ConnectionTarget<'_> {
    pub fn dir(&self) -> &str {
        match self {
            ConnectionTarget::Loose(p) => &p.dir,
            ConnectionTarget::Member(_, m) => &m.dir,
        }
    }

    pub fn role(&self) -> Option<&str> {
        match self {
            ConnectionTarget::Loose(p) => p.role.as_deref(),
            ConnectionTarget::Member(..) => None,
        }
    }

    pub fn check(&self, facts: &ConnectionFacts) -> Vec<ConnectionViolation> {
        let dir = self.dir();
        let role = self.role();
        let mut out = vec![];
        if dir != "input" && dir != "output" {
            out.push(ConnectionViolation::InvalidDirection(dir.to_string()));
            return out;
        }
        if dir == "input" && !facts.input {
            out.push(ConnectionViolation::NotInput);
        }
        if dir == "output" && !facts.drivable {
            out.push(ConnectionViolation::NotDrivable);
        }
        if role == Some("clock") && !facts.is_clock {
            out.push(ConnectionViolation::NotClock);
        }
        if role == Some("reset") && !facts.is_reset {
            out.push(ConnectionViolation::NotReset);
        }
        if facts.is_clock && role != Some("clock") {
            out.push(ConnectionViolation::ClockUndeclared);
        }
        if facts.is_reset && role != Some("reset") {
            out.push(ConnectionViolation::ResetUndeclared);
        }
        out
    }
}

#[derive(Clone)]
pub struct ManifestParam {
    pub name: String,
    pub ty: String,
    /// A parameter that may be left unset (`Option<T>` on the component).
    pub optional: bool,
    pub doc: Option<String>,
}

#[derive(Clone)]
pub struct ManifestMethod {
    pub name: String,
    pub args: Vec<ManifestParam>,
    /// Manifest type string of the return value; `None` for a unit return.
    pub ret: Option<String>,
    /// Declared width expression of a `value` return; see
    /// [`eval_width_expr`].
    pub ret_width: Option<WidthExpr>,
    pub doc: Option<String>,
}

/// A method width expression, stored structured in the manifest rather than
/// as source text: an integer, a component parameter name, a group-qualified
/// interface parameter (`axi.DATA_WIDTH_BYTES`), or `+ - * /` arithmetic
/// over them. Emitted by the `#[component]` macro from the already-parsed
/// Rust expression, so the metadata side needs no parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidthExpr {
    Num(u64),
    Param(String),
    /// A constant visible in the interface a group is bound to,
    /// resolved against the connected interface instance.
    GroupParam {
        group: String,
        name: String,
    },
    BinOp {
        op: char,
        lhs: Box<WidthExpr>,
        rhs: Box<WidthExpr>,
    },
}

impl std::fmt::Display for WidthExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_prec(f, 0)
    }
}

impl WidthExpr {
    /// Collects the group-qualified parameters this expression references.
    pub fn group_refs(&self, out: &mut Vec<(String, String)>) {
        match self {
            WidthExpr::GroupParam { group, name } => out.push((group.clone(), name.clone())),
            WidthExpr::BinOp { lhs, rhs, .. } => {
                lhs.group_refs(out);
                rhs.group_refs(out);
            }
            _ => (),
        }
    }

    fn precedence(&self) -> u8 {
        match self {
            WidthExpr::BinOp { op: '+' | '-', .. } => 1,
            WidthExpr::BinOp { .. } => 2,
            _ => 3,
        }
    }

    // Parenthesizes a subexpression only when its operator binds looser than
    // the enclosing one, so `WIDTH * 2 + 8` and `(WIDTH + DEPTH) * 2` both
    // round-trip readably in diagnostics.
    fn fmt_prec(&self, f: &mut std::fmt::Formatter<'_>, parent: u8) -> std::fmt::Result {
        match self {
            WidthExpr::Num(n) => write!(f, "{n}"),
            WidthExpr::Param(name) => write!(f, "{name}"),
            WidthExpr::GroupParam { group, name } => write!(f, "{group}.{name}"),
            WidthExpr::BinOp { op, lhs, rhs } => {
                let prec = self.precedence();
                if prec < parent {
                    write!(f, "(")?;
                }
                lhs.fmt_prec(f, prec)?;
                write!(f, " {op} ")?;
                rhs.fmt_prec(f, prec + 1)?;
                if prec < parent {
                    write!(f, ")")?;
                }
                Ok(())
            }
        }
    }
}

/// One type's declared interface. Parsed leniently: unknown fields are
/// ignored, so a manifest can gain fields without breaking existing readers.
#[derive(Clone, Default)]
pub struct ComponentManifest {
    /// Declared shape (`clocked`/`method_only`), injected into the sidecar
    /// from the vtable at extraction time; absent in the embedded JSON.
    pub kind: Option<String>,
    /// Component-level description from the type's doc comment.
    pub doc: Option<String>,
    pub ports: Vec<ManifestPort>,
    pub params: Vec<ManifestParam>,
    pub methods: Vec<ManifestMethod>,
    pub requires: Vec<String>,
    /// Interface bindings; empty for a component that declares only loose
    /// ports.
    pub groups: Vec<ManifestGroup>,
}

fn str_of(v: &Json, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn doc_of(v: &Json) -> Option<String> {
    v["doc"].as_str().map(str::to_string)
}

fn parse_params(v: &Json) -> Vec<ManifestParam> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|p| ManifestParam {
                    name: str_of(p, "name"),
                    ty: str_of(p, "type"),
                    optional: p["optional"].as_bool().unwrap_or(false),
                    doc: doc_of(p),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses a structured width expression from its manifest JSON encoding: a
/// number literal, a string parameter name (dotted for a group-qualified
/// interface parameter — Rust identifiers cannot contain `.`), or
/// `{"op":..,"lhs":..,"rhs":..}`. `None` for any other shape.
pub fn parse_width_expr(v: &Json) -> Option<WidthExpr> {
    if let Some(n) = v.as_u64() {
        Some(WidthExpr::Num(n))
    } else if let Some(name) = v.as_str() {
        Some(match name.split_once('.') {
            Some((group, name)) => WidthExpr::GroupParam {
                group: group.to_string(),
                name: name.to_string(),
            },
            None => WidthExpr::Param(name.to_string()),
        })
    } else if let Some(obj) = v.as_object() {
        let op = obj.get("op")?.as_str()?.chars().next()?;
        let lhs = parse_width_expr(obj.get("lhs")?)?;
        let rhs = parse_width_expr(obj.get("rhs")?)?;
        Some(WidthExpr::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    } else {
        None
    }
}

/// Evaluates a declared width expression against the instance's resolved
/// parameter values (group-qualified names are keyed `"group.name"`).
/// `None` when it references an unknown parameter or an operation
/// overflows.
pub fn eval_width_expr(expr: &WidthExpr, params: &[(String, u64)]) -> Option<u64> {
    match expr {
        WidthExpr::Num(n) => Some(*n),
        WidthExpr::Param(name) => params.iter().find(|(n, _)| n == name).map(|(_, v)| *v),
        WidthExpr::GroupParam { group, name } => params
            .iter()
            .find(|(n, _)| n.split_once('.') == Some((group, name)))
            .map(|(_, v)| *v),
        WidthExpr::BinOp { op, lhs, rhs } => {
            let lhs = eval_width_expr(lhs, params)?;
            let rhs = eval_width_expr(rhs, params)?;
            match op {
                '+' => lhs.checked_add(rhs),
                '-' => lhs.checked_sub(rhs),
                '*' => lhs.checked_mul(rhs),
                '/' => lhs.checked_div(rhs),
                _ => None,
            }
        }
    }
}

impl ComponentManifest {
    fn from_json(v: &Json) -> Self {
        let ports = v["ports"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|p| ManifestPort {
                        name: str_of(p, "name"),
                        dir: str_of(p, "dir"),
                        role: p["role"].as_str().map(str::to_string),
                        doc: doc_of(p),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let methods = v["methods"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| ManifestMethod {
                        name: str_of(m, "name"),
                        args: parse_params(&m["args"]),
                        ret: m["ret"].as_str().map(str::to_string),
                        ret_width: parse_width_expr(&m["ret_width"]),
                        doc: doc_of(m),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let requires = v["requires"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let groups = v["groups"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|g| ManifestGroup {
                        name: str_of(g, "name"),
                        interface: str_of(g, "interface"),
                        modport: str_of(g, "modport"),
                        members: g["members"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .map(|m| ManifestMember {
                                        member: str_of(m, "member"),
                                        dir: str_of(m, "dir"),
                                        doc: doc_of(m),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        doc: doc_of(g),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kind: v["kind"].as_str().map(str::to_string),
            doc: doc_of(v),
            ports,
            params: parse_params(&v["params"]),
            methods,
            requires,
            groups,
        }
    }

    /// Parses one type's manifest from a per-type JSON object.
    pub fn parse(json: &str) -> Option<Self> {
        let v: Json = serde_json::from_str(json).ok()?;
        Some(Self::from_json(&v))
    }

    /// Extracts one type's manifest from a library-level aggregated JSON
    /// (`{"types":{"<name>":{...}}}`).
    pub fn parse_from_library(json: &str, type_name: &str) -> Option<Self> {
        let v: Json = serde_json::from_str(json).ok()?;
        let entry = v.get("types")?.get(type_name)?;
        Some(Self::from_json(entry))
    }

    pub fn port(&self, name: &str) -> Option<&ManifestPort> {
        self.ports.iter().find(|p| p.name == name)
    }

    /// What a connection binds to: a modport-expanded member binds by its
    /// (group, member) declaration, anything else to a loose port by name.
    /// This is the binding contract shared by the analyzer's connection
    /// checks and the simulator's load-time wiring; a member the manifest
    /// does not declare resolves to nothing (the component ignores that
    /// interface member).
    pub fn connection_target(
        &self,
        name: &str,
        group: Option<&str>,
        member: Option<&str>,
    ) -> Option<ConnectionTarget<'_>> {
        match (group, member) {
            (Some(g), Some(m)) => {
                let grp = self.groups.iter().find(|x| x.name == g)?;
                let member = grp.members.iter().find(|x| x.member == m)?;
                Some(ConnectionTarget::Member(grp, member))
            }
            _ => self.port(name).map(ConnectionTarget::Loose),
        }
    }

    pub fn param(&self, name: &str) -> Option<&ManifestParam> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn method(&self, name: &str) -> Option<&ManifestMethod> {
        self.methods.iter().find(|m| m.name == name)
    }

    /// Extracts every exported type's manifest from a prebuilt wasm
    /// binary's `veryl.manifest` custom section (embedded at build time by
    /// `veryl_component_export!`), without a wasm runtime. This is what
    /// makes a fresh checkout analyzable before anything is built.
    pub fn parse_all_from_wasm(wasm: &[u8]) -> Option<HashMap<String, Self>> {
        let payload = crate::wasm_section::wasm_custom_section(
            wasm,
            veryl_component_sys::VRL_WASM_MANIFEST_SECTION,
        )?;
        let json = std::str::from_utf8(payload).ok()?;
        let manifests = parse_library_manifest(json);
        (!manifests.is_empty()).then_some(manifests)
    }
}

impl ManifestMethod {
    /// Signature suffix of the return value (`" -> u64"`,
    /// `" -> value[WIDTH]"`); empty for unit.
    pub fn ret_suffix(&self) -> String {
        match (&self.ret, &self.ret_width) {
            (Some(t), Some(w)) => format!(" -> {t}[{w}]"),
            (Some(t), None) => format!(" -> {t}"),
            (None, _) => String::new(),
        }
    }
}

pub fn parse_library_manifest(json: &str) -> HashMap<String, ComponentManifest> {
    let mut ret = HashMap::new();
    let Ok(v) = serde_json::from_str::<Json>(json) else {
        return ret;
    };
    if let Some(types) = v.get("types").and_then(|t| t.as_object()) {
        for (name, entry) in types {
            ret.insert(name.clone(), ComponentManifest::from_json(entry));
        }
    }
    ret
}

/// Filename of the committed interface-manifest sidecar in a component
/// crate directory. Unlike the build-output sidecar under `target/`, this
/// one is committed, so a fresh checkout (or a source-only dependency) is
/// analyzable without building the component. It may carry `source_hash`
/// and `veryl_version` fields beside `types`; the reader ignores them.
pub const COMMITTED_MANIFEST_FILE: &str = "veryl.manifest.json";

#[cfg(test)]
mod committed_manifest_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn committed_manifest_enumeration_ignores_stamp() {
        let dir = std::env::temp_dir().join(format!("veryl_committed_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(COMMITTED_MANIFEST_FILE),
            r#"{"types":{"widget":{"kind":"clocked","ports":[{"name":"clk","dir":"input","width":1,"role":"clock"}]}},"source_hash":"deadbeef"}"#,
        )
        .unwrap();

        let entry = crate::Component {
            path: ".".into(),
            wasm: None,
        };
        let manifests = entry.collect_manifests(&dir, Path::new("/no/such/target"));
        assert_eq!(manifests.len(), 1);
        let (name, m) = &manifests[0];
        assert_eq!(name, "widget");
        assert_eq!(m.kind.as_deref(), Some("clocked"));
        assert_eq!(m.port("clk").unwrap().role.as_deref(), Some("clock"));

        assert!(
            entry
                .collect_manifests(Path::new("/no/such/dir"), Path::new("/no/such/target"))
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
