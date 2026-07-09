//! Resolves component type names to ABI vtables.
//!
//! Two sources: dynamic libraries built from the user's Rust components
//! (loaded once per process and never unloaded), and an in-process static
//! registry that bypasses dlopen — used by tests and available for builtin
//! components.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use thiserror::Error;
use veryl_component_sys as sys;

#[derive(Error, Debug)]
pub enum ComponentError {
    #[error("failed to load component library {path}: {reason}")]
    LibraryLoad { path: PathBuf, reason: String },
    #[error("{path} does not export `{symbol}`: not a Veryl component library", symbol = sys::VRL_LOOKUP_SYMBOL)]
    MissingLookup { path: PathBuf },
    #[error("component type `{name}` not found{}{}", in_library(.path), exports_hint(.available))]
    UnknownType {
        name: String,
        path: Option<PathBuf>,
        /// Export names the library declares in its manifest, when it has
        /// one; shown so a key/export mismatch is visible from the error.
        available: Vec<String>,
    },
    #[error(
        "component `{name}` was built for ABI version {found}, host requires {expected}; rebuild it against the matching veryl-component"
    )]
    AbiMismatch {
        name: String,
        found: u32,
        expected: u32,
    },
    #[error("component failed to initialize: {messages}")]
    CreateFailed { messages: String },
    #[error("dynamic component libraries are not supported on this platform")]
    Unsupported,
    #[error(
        "component library {path} is a wasm binary, but this veryl build targets a wasm platform and cannot host wasm components; use a native veryl build instead"
    )]
    WasmHostUnsupported { path: PathBuf },
    #[error(
        "component `{name}` declares `requires(native)` and cannot run as a prebuilt wasm binary; build it from source instead"
    )]
    WasmNativeComponent { name: String },

    // --- instance build/validation errors (from `build_components`) ---
    // Each carries the instance name so the message is self-contained.
    #[error("component `{inst}`: {source}")]
    Instance {
        inst: String,
        #[source]
        source: Box<ComponentError>,
    },
    #[error("component `{inst}`: manifest of `{type_name}` cannot be parsed")]
    ManifestParse { inst: String, type_name: String },
    #[error("component `{inst}`: {reason}")]
    ManifestInvalid { inst: String, reason: String },
    #[error("component `{inst}`: `{type_name}` declares no port named `{port}`")]
    UnknownPort {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error("component `{inst}`: port `{port}` of `{type_name}` is connected more than once")]
    PortMultiplyConnected {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error("component `{inst}`: `{type_name}` declares no interface port named `{group}`")]
    UnknownGroup {
        inst: String,
        type_name: String,
        group: String,
    },
    #[error(
        "component `{inst}`: port `{port}` of `{type_name}` has invalid direction `{dir}` in its manifest"
    )]
    InvalidPortDirection {
        inst: String,
        type_name: String,
        port: String,
        dir: String,
    },
    #[error(
        "component `{inst}`: port `{port}` of `{type_name}` is declared as an input but the modport connection only allows driving it"
    )]
    PortNotInput {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: port `{port}` of `{type_name}` is declared as an output but the connection cannot be driven"
    )]
    PortNotDrivable {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: port `{port}` of `{type_name}` is declared as a clock but the connected expression is not a clock"
    )]
    PortNotClock {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: port `{port}` of `{type_name}` is declared as a reset but the connected expression is not a reset"
    )]
    PortNotReset {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: the `{port}` connection is a clock but `{type_name}` does not declare the port as a clock (a ClockPort field)"
    )]
    ClockPortUndeclared {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: the `{port}` connection is a reset but `{type_name}` does not declare the port as a reset (a ResetPort field)"
    )]
    ResetPortUndeclared {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error("component `{inst}`: {role} port `{port}` of `{type_name}` is not connected")]
    RolePortUnconnected {
        inst: String,
        type_name: String,
        port: String,
        role: String,
    },
    #[error("component `{inst}`: `{type_name}` declares no parameter named `{param}`")]
    UnknownParam {
        inst: String,
        type_name: String,
        param: String,
    },
    #[error(
        "component `{inst}`: `{type_name}` is a clocked component; declare it with `inst`, not `var`"
    )]
    ClockedNeedsInst { inst: String, type_name: String },
    #[error(
        "component `{inst}`: `{type_name}` is a method-only component; declare it with `var`, not `inst`"
    )]
    MethodOnlyNeedsVar { inst: String, type_name: String },
    #[error("component `{inst}`: cannot determine the width of the `{port}` connection")]
    UndeterminedWidth { inst: String, port: String },
    #[error(
        "component `{inst}`: cannot determine the {role} event source of the `{port}` connection"
    )]
    UndeterminedEventSource {
        inst: String,
        port: String,
        role: String,
    },
    #[error(
        "component `{inst}`: the `{port}` connection is a {role} but `{type_name}` does not declare the port as one (a {field} field)"
    )]
    RoleNotResolved {
        inst: String,
        type_name: String,
        port: String,
        role: String,
        field: String,
    },
    #[error(
        "component `{inst}`: `{type_name}` is a clocked component but resolved no clock port to fire it"
    )]
    NoClockPortResolved { inst: String, type_name: String },
    #[error("component `{inst}`: `{type_name}` does not use a port named `{port}`")]
    PortUnused {
        inst: String,
        type_name: String,
        port: String,
    },
    #[error(
        "component `{inst}`: `{type_name}` does not use any member of the `{group}` connection"
    )]
    GroupUnused {
        inst: String,
        type_name: String,
        group: String,
    },
    #[error("component `{inst}`: output to `{var}` conflicts with an RTL driver")]
    OutputRtlConflict { inst: String, var: String },
    #[error("component `{inst}`: output to `{var}` conflicts with component `{other}`")]
    OutputComponentConflict {
        inst: String,
        var: String,
        other: String,
    },
    #[error("{messages}")]
    InitFailed { messages: String },
}

/// A resolved component implementation behind one of the transports.
pub enum ComponentBackend {
    Native(&'static sys::VrlComponentVTable),
    #[cfg(not(target_family = "wasm"))]
    Wasm {
        library: std::sync::Arc<crate::component::wasm::WasmLibrary>,
        type_name: String,
        kind: u32,
        file_allowed: bool,
    },
}

impl ComponentBackend {
    /// The declared `VRL_KIND_*`, available before instantiation.
    pub fn kind(&self) -> u32 {
        match self {
            ComponentBackend::Native(vtable) => vtable.kind,
            #[cfg(not(target_family = "wasm"))]
            ComponentBackend::Wasm { kind, .. } => *kind,
        }
    }
}

impl std::fmt::Debug for ComponentBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentBackend::Native(_) => write!(f, "ComponentBackend::Native"),
            #[cfg(not(target_family = "wasm"))]
            ComponentBackend::Wasm {
                type_name, kind, ..
            } => f
                .debug_struct("ComponentBackend::Wasm")
                .field("type_name", type_name)
                .field("kind", kind)
                .finish(),
        }
    }
}

impl From<&'static sys::VrlComponentVTable> for ComponentBackend {
    fn from(vtable: &'static sys::VrlComponentVTable) -> Self {
        ComponentBackend::Native(vtable)
    }
}

pub fn is_wasm_library(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "wasm")
}

/// Resolves a component type to its transport backend: `.wasm` libraries
/// go to the wasm runtime, everything else follows [`lookup_component`].
pub fn lookup_component_backend(
    library: Option<&Path>,
    type_name: &str,
) -> Result<ComponentBackend, ComponentError> {
    match library {
        Some(path) if is_wasm_library(path) => {
            #[cfg(not(target_family = "wasm"))]
            {
                crate::component::wasm::lookup_wasm_component(path, type_name)
            }
            #[cfg(target_family = "wasm")]
            {
                let _ = type_name;
                Err(ComponentError::WasmHostUnsupported {
                    path: path.to_path_buf(),
                })
            }
        }
        _ => Ok(ComponentBackend::Native(lookup_component(
            library, type_name,
        )?)),
    }
}

fn in_library(path: &Option<PathBuf>) -> String {
    match path {
        Some(path) => format!(" in {}", path.display()),
        None => String::new(),
    }
}

fn exports_hint(available: &[String]) -> String {
    if available.is_empty() {
        return String::new();
    }
    let names: Vec<_> = available.iter().map(|n| format!("`{n}`")).collect();
    format!(
        "; the library exports {} — components are instantiated by these names",
        names.join(", ")
    )
}

/// Export names listed in a library's aggregated manifest, for the
/// unknown-type diagnostic. Best-effort: a hand-written library without a
/// manifest yields an empty list.
pub(crate) fn library_export_names(path: &Path) -> Vec<String> {
    let Some(json) = library_manifest(path) else {
        return vec![];
    };
    let mut ret: Vec<String> = veryl_metadata::parse_library_manifest(&json)
        .into_keys()
        .collect();
    ret.sort();
    ret
}

/// The `&'static` in the value is sound because registered vtables either
/// live in a leaked (never-unloaded) library or in guest `static` tables.
static STATIC_REGISTRY: LazyLock<Mutex<HashMap<String, &'static sys::VrlComponentVTable>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register_static_component(name: &str, vtable: &'static sys::VrlComponentVTable) {
    STATIC_REGISTRY
        .lock()
        .unwrap()
        .insert(name.to_string(), vtable);
}

/// Per-type manifest JSON registered alongside static components.
static STATIC_MANIFESTS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register_static_manifest(name: &str, json: &str) {
    STATIC_MANIFESTS
        .lock()
        .unwrap()
        .insert(name.to_string(), json.to_string());
}

pub fn static_manifest(type_name: &str) -> Option<String> {
    STATIC_MANIFESTS.lock().unwrap().get(type_name).cloned()
}

/// Aggregated manifest JSON (`{"types":{...}}`) of a component library:
/// the manifest symbol of a dynamic library, or the `veryl.manifest`
/// custom section of a wasm binary (read without a wasm runtime, so it
/// works regardless of the transport feature). `None` when the library
/// carries no manifest or cannot be loaded (the load error surfaces via
/// `lookup_component`).
pub fn library_manifest(path: &Path) -> Option<String> {
    if is_wasm_library(path) {
        return wasm_library_manifest(path);
    }
    native_library_manifest(path)
}

#[cfg(not(target_family = "wasm"))]
fn native_library_manifest(path: &Path) -> Option<String> {
    let library = get_library(path).ok()?;
    let manifest: libloading::Symbol<sys::VrlManifestFn> =
        unsafe { library.get(sys::VRL_MANIFEST_SYMBOL.as_bytes()) }.ok()?;
    let s = unsafe { manifest() };
    Some(unsafe { s.as_str() }.to_string())
}

#[cfg(target_family = "wasm")]
fn native_library_manifest(_path: &Path) -> Option<String> {
    None
}

fn wasm_library_manifest(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let payload = veryl_metadata::wasm_custom_section(&bytes, sys::VRL_WASM_MANIFEST_SECTION)?;
    String::from_utf8(payload.to_vec()).ok()
}

/// Extracts one type's manifest from a library-level aggregated JSON
/// document, failing closed: a present-but-unparseable document is an
/// error, while a type absent from a valid document simply carries no
/// manifest.
pub(crate) fn parse_library_manifest_json(
    json: &str,
    type_name: &str,
) -> Result<Option<veryl_metadata::ComponentManifest>, String> {
    let doc: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("component manifest is not valid JSON: {e}"))?;
    let Some(types) = doc.get("types").and_then(|t| t.as_object()) else {
        return Err("component manifest has no `types` object".to_string());
    };
    if !types.contains_key(type_name) {
        return Ok(None);
    }
    match veryl_metadata::ComponentManifest::parse_from_library(json, type_name) {
        Some(manifest) => Ok(Some(manifest)),
        None => Err(format!(
            "component manifest entry for `{type_name}` cannot be parsed"
        )),
    }
}

/// Resolves a component type. `library == None` consults the static
/// registry; `Some(path)` loads (or reuses) the dynamic library and calls
/// its lookup symbol. The ABI version is checked in both paths.
pub fn lookup_component(
    library: Option<&Path>,
    type_name: &str,
) -> Result<&'static sys::VrlComponentVTable, ComponentError> {
    let vtable = match library {
        None => STATIC_REGISTRY
            .lock()
            .unwrap()
            .get(type_name)
            .copied()
            .ok_or_else(|| ComponentError::UnknownType {
                name: type_name.to_string(),
                path: None,
                available: vec![],
            })?,
        Some(path) => lookup_in_library(path, type_name)?,
    };
    if vtable.abi_version != sys::VRL_COMPONENT_ABI_VERSION {
        return Err(ComponentError::AbiMismatch {
            name: type_name.to_string(),
            found: vtable.abi_version,
            expected: sys::VRL_COMPONENT_ABI_VERSION,
        });
    }
    Ok(vtable)
}

/// Loads (or reuses) a dynamic library. Libraries are leaked
/// intentionally: instances may outlive any local scope and unloading
/// component code mid-process is never safe.
#[cfg(not(target_family = "wasm"))]
fn get_library(path: &Path) -> Result<&'static libloading::Library, ComponentError> {
    static LIBRARIES: LazyLock<Mutex<HashMap<PathBuf, &'static libloading::Library>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let mut libraries = LIBRARIES.lock().unwrap();
    match libraries.get(path) {
        Some(library) => Ok(*library),
        None => {
            let library = unsafe { libloading::Library::new(path) }.map_err(|e| {
                ComponentError::LibraryLoad {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                }
            })?;
            let library: &'static libloading::Library = Box::leak(Box::new(library));
            libraries.insert(path.to_path_buf(), library);
            Ok(library)
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn lookup_in_library(
    path: &Path,
    type_name: &str,
) -> Result<&'static sys::VrlComponentVTable, ComponentError> {
    let library = get_library(path)?;

    let lookup: libloading::Symbol<sys::VrlLookupFn> = unsafe {
        library.get(sys::VRL_LOOKUP_SYMBOL.as_bytes())
    }
    .map_err(|_| ComponentError::MissingLookup {
        path: path.to_path_buf(),
    })?;
    let vtable = unsafe { lookup(sys::VrlStr::from_str(type_name)) };
    if vtable.is_null() {
        return Err(ComponentError::UnknownType {
            name: type_name.to_string(),
            path: Some(path.to_path_buf()),
            available: library_export_names(path),
        });
    }
    Ok(unsafe { &*vtable })
}

#[cfg(target_family = "wasm")]
fn lookup_in_library(
    _path: &Path,
    _type_name: &str,
) -> Result<&'static sys::VrlComponentVTable, ComponentError> {
    Err(ComponentError::Unsupported)
}
