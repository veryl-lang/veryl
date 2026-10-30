//! Records of component conversions, replayed in place of converting a
//! component whose inputs did not change.
//!
//! A record holds what the conversion did to the diagnostics, in order: the
//! component's own diagnostics and each point where it reached a child. A
//! child's diagnostics belong to the child's record, so replaying a record
//! visits the children as converting it would, and the memo, the hierarchy
//! limits and the instance notes come out as they would.

use crate::AnalyzerError;
use crate::HashMap;
use crate::HashSet;
use crate::comb_loop_detect::UnitAnalysis;
use crate::fragment_cache::FileWindow;
use crate::ir::{Component, Declaration, Signature};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::Arc;
use veryl_parser::fragment_codec as parser_codec;
use veryl_parser::fragment_codec::IdRebase;
use veryl_parser::resource_table::{PathId, StrId, TokenId};
use veryl_parser::text_table;
use veryl_parser::token_range::TokenRange;
use veryl_parser::veryl_token::{Token, TokenSource};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UnitEvent {
    Error(AnalyzerError),
    Child(ChildReach),
    EvalOverflow(TokenRange, usize),
    ForOverflow(TokenRange),
    /// An inferred variable type entered in `resolved_type_table`, which
    /// other signatures of the definition are checked against.
    /// With the type, in `stable_type` form.
    ResolvedType(TokenId, String),
    /// A reference the body makes without naming the symbol, which
    /// `symbol_table` keeps for the unused-variable check: the symbol by its
    /// declaration, and where.
    Reference(Token, Token),
}

/// A copy of `error` for a record, without the whole text of its file.
pub(crate) fn record_copy(error: &mut AnalyzerError) -> AnalyzerError {
    let (path, len) = match error.token_source() {
        source @ TokenSource::File { text, .. } => {
            (source.to_string(), text_table::text_len(text).unwrap_or(0))
        }
        _ => return error.clone(),
    };
    let sources = std::mem::take(&mut error.input_sources_mut().sources);
    let mut copy = error.clone();
    copy.input_sources_mut().sources = sources
        .iter()
        .map(|x| x.without_file_text(&path, len))
        .collect();
    error.input_sources_mut().sources = sources;
    copy
}

/// `error` from a record, with the text of its file restored.
pub(crate) fn restore_file_text(mut error: AnalyzerError) -> AnalyzerError {
    let source = error.token_source();
    for x in &mut error.input_sources_mut().sources {
        if x.is_file_text_left_out() {
            x.restore_file_text(source.get_text());
        }
    }
    error
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildReach {
    pub key: String,
    pub overridden: String,
    pub has_overrides: bool,
    pub site: Option<TokenRange>,
    /// Where a hierarchy limit hit by the reach is reported.
    pub token: TokenRange,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnitRecord {
    /// The file defining the component.
    pub file: PathId,
    /// Other files the conversion read: those defining what the generic
    /// arguments name.
    pub inputs: Vec<PathId>,
    pub events: Vec<UnitEvent>,
    /// Whether the conversion succeeded, which memoizes it.
    pub ok: bool,
    /// Whether parents read more than the header: an interface, or a module
    /// driving the interfaces connected to it.
    pub needs_body: bool,
}

#[derive(Default)]
struct UnitCache {
    records: HashMap<String, Arc<UnitRecord>>,
    /// The combinational-loop analysis of the module units.
    analyses: HashMap<String, UnitAnalysis>,
    /// Where each file's tokens and text are numbered this run.
    windows: HashMap<PathId, FileWindow>,
    /// Files whose units changed this run.
    dirty: HashSet<PathId>,
    /// Converted from the header alone this run, their analysis replayed.
    stubs: HashMap<Signature, String>,
    /// Stubs converted whole since, for a reader of their bodies.
    whole: HashSet<Signature>,
    /// The header-alone components themselves, by address: a parent keeps the
    /// one it was given after the memo moves on to a whole conversion. Held
    /// so the addresses stay theirs.
    stub_components: HashMap<usize, Arc<Component>>,
    /// Units to convert rather than replay: a replay is missing a result.
    forced: HashSet<String>,
    requested: HashSet<String>,
    /// Inferred types a replay skipped entering, with the units to convert
    /// if a conversion checks against one.
    replayed_types: HashMap<TokenId, (String, Vec<String>)>,
    recording: bool,
    /// Files whose components may be replayed; `None` replays nothing.
    reusable_files: Option<HashSet<PathId>>,
    replayable: HashMap<String, bool>,
}

thread_local!(static UNIT_CACHE: RefCell<UnitCache> = RefCell::new(UnitCache::default()));

pub fn set_recording(recording: bool) {
    UNIT_CACHE.with(|f| f.borrow_mut().recording = recording);
}

pub fn is_recording() -> bool {
    UNIT_CACHE.with(|f| f.borrow().recording)
}

pub fn set_reusable_files(files: Option<HashSet<PathId>>) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        f.reusable_files = files;
        f.replayable.clear();
    });
}

pub fn is_active() -> bool {
    UNIT_CACHE.with(|f| {
        let f = f.borrow();
        f.recording || f.reusable_files.is_some()
    })
}

pub fn insert(key: String, record: UnitRecord) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        f.dirty.insert(record.file);
        f.analyses.remove(&key);
        // Replay is decided at the first reach, so other memoized answers
        // were already acted on.
        f.replayable.remove(&key);
        f.records.insert(key, Arc::new(record));
    });
}

/// Drops the record of `key`, which its latest conversion left uncacheable.
pub fn remove(key: &str, file: PathId) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        if f.records.remove(key).is_some() {
            f.dirty.insert(file);
        }
        f.analyses.remove(key);
        f.replayable.remove(key);
    });
}

pub(crate) fn analysis(key: &str) -> Option<UnitAnalysis> {
    UNIT_CACHE.with(|f| f.borrow().analyses.get(key).cloned())
}

pub(crate) fn update_analysis(key: &str, update: impl FnOnce(&mut UnitAnalysis)) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        let f = &mut *f;
        if let Some(record) = f.records.get(key) {
            f.dirty.insert(record.file);
        }
        update(f.analyses.entry(key.to_string()).or_default());
    });
}

pub fn set_file_window(file: PathId, window: FileWindow) {
    UNIT_CACHE.with(|f| f.borrow_mut().windows.insert(file, window));
}

#[derive(Serialize, Deserialize)]
struct FileUnits {
    strings: Vec<String>,
    paths: Vec<std::path::PathBuf>,
    units: Vec<(String, Vec<u8>)>,
}

/// The units defined in `file` changed this run, serialized; `None` when
/// they did not change. A unit referring to anything numbered outside the
/// file is left out, which leaves it to be converted.
pub fn encode_file(file: PathId) -> Option<Vec<u8>> {
    UNIT_CACHE.with(|f| {
        let f = f.borrow();
        if !f.dirty.contains(&file) {
            return None;
        }
        let window = f.windows.get(&file)?;
        let mut keys: Vec<_> = f
            .records
            .iter()
            .filter(|(_, x)| x.file == file)
            .map(|(key, _)| key)
            .collect();
        keys.sort();
        parser_codec::begin_encode(parser_codec::EncodeSession::new(window.token, window.text));
        let units = keys
            .into_iter()
            .filter_map(|key| {
                let unit = (&*f.records[key], f.analyses.get(key));
                postcard::to_allocvec(&unit)
                    .ok()
                    .map(|bytes| (key.clone(), bytes))
            })
            .collect();
        let dicts = parser_codec::end_encode()?;
        postcard::to_allocvec(&FileUnits {
            strings: dicts.strings,
            paths: dicts.paths,
            units,
        })
        .ok()
    })
}

/// Loads the units a previous run encoded for `file`, numbered as the file
/// is this run.
pub fn decode_file(file: PathId, bytes: &[u8]) {
    let Ok(units) = postcard::from_bytes::<FileUnits>(bytes) else {
        return;
    };
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        let Some(window) = f.windows.get(&file).copied() else {
            return;
        };
        parser_codec::begin_decode(parser_codec::DecodeSession::new(
            &units.strings,
            &units.paths,
            IdRebase {
                base: window.token.start,
                count: window.token.count(),
            },
            IdRebase {
                base: window.text.start,
                count: window.text.count(),
            },
        ));
        for (key, bytes) in units.units {
            let decoded = postcard::from_bytes::<(UnitRecord, Option<UnitAnalysis>)>(&bytes);
            if let Ok((record, analysis)) = decoded
                && record.file == file
            {
                if let Some(analysis) = analysis {
                    f.analyses.insert(key.clone(), analysis);
                }
                f.records.insert(key, Arc::new(record));
            }
        }
        parser_codec::end_decode();
        f.replayable.clear();
    });
}

pub fn get(key: &str) -> Option<Arc<UnitRecord>> {
    UNIT_CACHE.with(|f| f.borrow().records.get(key).cloned())
}

/// Whether the record of `key` and the records of everything it reaches are
/// present and defined in reusable files.
pub fn is_replayable(key: &str) -> bool {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        let f = &mut *f;
        let Some(reusable) = &f.reusable_files else {
            return false;
        };
        replayable(
            key,
            &f.records,
            reusable,
            &f.forced,
            &mut f.replayable,
            &mut HashSet::default(),
        )
    })
}

fn replayable(
    key: &str,
    records: &HashMap<String, Arc<UnitRecord>>,
    reusable: &HashSet<PathId>,
    forced: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(x) = memo.get(key) {
        return *x;
    }
    // A cycle is a recursive instantiation, which the replay reports as the
    // conversion did.
    if !visiting.insert(key.to_string()) {
        return true;
    }
    let ret = records.get(key).is_some_and(|record| {
        reusable.contains(&record.file)
            && record.inputs.iter().all(|x| reusable.contains(x))
            && !forced.contains(key)
            && record.events.iter().all(|event| match event {
                UnitEvent::Child(x) => {
                    replayable(&x.key, records, reusable, forced, memo, visiting)
                }
                _ => true,
            })
    });
    visiting.remove(key);
    memo.insert(key.to_string(), ret);
    ret
}

pub fn register_stub(signature: Signature, key: String, component: &Arc<Component>) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        f.stubs.insert(signature, key);
        f.stub_components
            .insert(Arc::as_ptr(component) as usize, Arc::clone(component));
    });
}

/// Components replayed from the header alone in the current pass, for
/// tests.
pub fn stub_count() -> usize {
    UNIT_CACHE.with(|f| f.borrow().stubs.len())
}

/// Whether the memo holds the header alone of `signature`.
pub fn is_header_only(signature: &Signature) -> bool {
    UNIT_CACHE.with(|f| {
        let f = f.borrow();
        f.stubs.contains_key(signature) && !f.whole.contains(signature)
    })
}

pub fn mark_whole(signature: Signature) {
    UNIT_CACHE.with(|f| f.borrow_mut().whole.insert(signature));
}

/// Whether `component`, or anything instantiated below it under `names`,
/// holds a header alone, as a test reading those bodies would find.
pub fn has_header_only_under(component: &Arc<Component>, names: &HashSet<StrId>) -> bool {
    fn visit(
        component: &Arc<Component>,
        names: &HashSet<StrId>,
        visited: &mut HashSet<usize>,
    ) -> bool {
        let address = Arc::as_ptr(component) as usize;
        if !visited.insert(address) {
            return false;
        }
        if UNIT_CACHE.with(|f| f.borrow().stub_components.contains_key(&address)) {
            return true;
        }
        let Component::Module(module) = component.as_ref() else {
            return false;
        };
        module.declarations.iter().any(|x| match x {
            Declaration::Inst(x) if names.contains(&x.name) => visit(&x.component, names, visited),
            _ => false,
        })
    }
    visit(component, names, &mut HashSet::default())
}

/// The key of `component` if it is a header alone, which a reader of its
/// body has to have converted again.
pub fn header_only_key(component: &Arc<Component>) -> Option<String> {
    UNIT_CACHE.with(|f| {
        let f = f.borrow();
        let address = Arc::as_ptr(component) as usize;
        if !f.stub_components.contains_key(&address) {
            return None;
        }
        let Component::Module(module) = component.as_ref() else {
            return None;
        };
        f.stubs.get(&module.signature).cloned()
    })
}

pub fn stub_key(signature: &Signature) -> Option<String> {
    UNIT_CACHE.with(|f| f.borrow().stubs.get(signature).cloned())
}

/// Asks for the units on `chain`, from a stub down, to be converted: a
/// result the analysis needs below the stub is missing.
pub fn request_conversion(chain: &[String]) {
    UNIT_CACHE.with(|f| f.borrow_mut().requested.extend(chain.iter().cloned()));
}

/// Notes that a replay skipped entering `inferred` as the type of `token`.
/// A second one for the same token would have been checked against the
/// first, so both are converted instead if they differ.
pub fn note_replayed_type(token: TokenId, inferred: String, chain: Vec<String>) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        if let Some((first, first_chain)) = f.replayed_types.get(&token).cloned() {
            if first != inferred {
                f.requested.extend(first_chain);
                f.requested.extend(chain);
            }
        } else {
            f.replayed_types.insert(token, (inferred, chain));
        }
    });
}

/// Asks for the units whose replay skipped entering a type of `token` other
/// than `inferred` to be converted, as a conversion is about to check
/// `inferred` against it.
pub fn check_replayed_type(token: &TokenId, inferred: &str) {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        if let Some((replayed, chain)) = f.replayed_types.get(token).cloned()
            && replayed != inferred
        {
            f.requested.extend(chain);
        }
    });
}

/// Forces the requested units to convert, for the caller to convert again.
pub fn force_requested() -> bool {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        let requested = std::mem::take(&mut f.requested);
        if requested.is_empty() {
            return false;
        }
        f.forced.extend(requested);
        f.replayable.clear();
        true
    })
}

/// Starts a conversion pass: the stubs of the previous one are gone.
pub fn begin_pass() {
    UNIT_CACHE.with(|f| {
        let mut f = f.borrow_mut();
        f.stubs.clear();
        f.whole.clear();
        f.stub_components.clear();
        f.requested.clear();
        f.replayed_types.clear();
    });
}

pub fn clear() {
    UNIT_CACHE.with(|f| *f.borrow_mut() = UnitCache::default());
}
