//! Per-file pass1 fragment capture/restore.
//!
//! A fragment holds everything one file's parse + pass1 wrote to the
//! global tables, serialized. Restoring it replaces parse + `analyze_pass1`
//! for an unchanged file; `analyze_post_pass1` then runs over the mix of
//! fresh and restored files. Capture must run before `analyze_post_pass1`,
//! which mutates symbols across files. IDs are normalized by the fragment
//! codec ([`veryl_parser::fragment_codec`]); a fragment referencing an ID
//! outside its own window (e.g. a clock domain in another file) is
//! non-cacheable.

use crate::analyzer_error::CachedDiagnostic;
use crate::attribute::Attribute;
use crate::definition_table::{self, Definition, DefinitionId};
use crate::fragment_codec;
use crate::generic_inference_table::{self, PendingEntry};
use crate::literal::Literal;
use crate::literal_table;
use crate::namespace::Namespace;
use crate::namespace_table;
use crate::reference_table::{self, ReferenceCandidate};
use crate::symbol;
use crate::symbol_table::{self, PendingWatermark, SymbolTableFragment};
use crate::type_dag::{self, TypeDagCandidate};
use crate::r#unsafe::Unsafe;
use crate::{attribute_table, unsafe_table};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use veryl_parser::doc_comment_table;
use veryl_parser::fragment_codec as parser_codec;
use veryl_parser::fragment_codec::{IdRebase, IdWindow};
use veryl_parser::resource_table::{self, StrId, TokenId};
use veryl_parser::text_table::{self, TextId, TextInfo};
use veryl_parser::token_range::TokenRange;

/// Counter values and pending-list lengths recorded before one file's
/// parse + pass1. Delimits the file's additions to the global tables.
#[derive(Clone, Copy, Debug)]
pub struct FragmentWatermark {
    token: usize,
    text: usize,
    symbol: usize,
    definition: usize,
    pending: PendingWatermark,
    reference_candidates: usize,
    type_dag_candidates: usize,
    generic_pending: usize,
}

/// Records the current global-table state. Call right before parsing a
/// file whose pass1 output should be captured.
pub fn watermark() -> FragmentWatermark {
    FragmentWatermark {
        token: resource_table::peek_token_id(),
        text: text_table::peek_text_id(),
        symbol: symbol::peek_symbol_id(),
        definition: definition_table::peek_definition_id(),
        pending: symbol_table::pending_watermark(),
        reference_candidates: reference_table::candidates_len(),
        type_dag_candidates: type_dag::candidates_len(),
        generic_pending: generic_inference_table::pending_len(),
    }
}

/// The ID-normalized part of a fragment, serialized with the fragment
/// codec sessions active.
#[derive(Serialize, Deserialize)]
struct FragmentPayload {
    doc_comments: Vec<(u32, StrId)>,
    symbols: SymbolTableFragment,
    namespace_entries: Vec<(TokenId, Namespace)>,
    literals: Vec<(TokenId, Literal)>,
    attributes: Vec<(TokenRange, Attribute)>,
    unsafes: Vec<(TokenRange, Unsafe)>,
    definitions: Vec<(DefinitionId, Definition)>,
    reference_candidates: Vec<ReferenceCandidate>,
    type_dag_candidates: Vec<TypeDagCandidate>,
    generic_inference_pending: Vec<PendingEntry>,
}

/// A self-contained, serializable pass1 fragment for one source file.
#[derive(Serialize, Deserialize)]
pub struct Fragment {
    pub src_path: PathBuf,
    pub source_text: String,
    strings: Vec<String>,
    paths: Vec<PathBuf>,
    token_count: usize,
    symbol_count: usize,
    definition_count: usize,
    payload: Vec<u8>,
}

impl Fragment {
    /// Serializes the fragment for on-disk storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>, FragmentError> {
        postcard::to_allocvec(self).map_err(|x| FragmentError::NonCacheable(x.to_string()))
    }

    /// Deserializes a fragment from on-disk storage.
    pub fn from_bytes(bytes: &[u8]) -> Result<Fragment, FragmentError> {
        postcard::from_bytes(bytes).map_err(|x| FragmentError::Restore(x.to_string()))
    }
}

/// Serializes one file's cached diagnostics. They are self-contained, so
/// unlike a fragment they need no ID codec session.
pub fn capture_diagnostics(diagnostics: &[CachedDiagnostic]) -> Result<Vec<u8>, FragmentError> {
    postcard::to_allocvec(diagnostics).map_err(|x| FragmentError::NonCacheable(x.to_string()))
}

/// Deserializes diagnostics serialized by [`capture_diagnostics`].
pub fn restore_diagnostics(bytes: &[u8]) -> Result<Vec<CachedDiagnostic>, FragmentError> {
    postcard::from_bytes(bytes).map_err(|x| FragmentError::Restore(x.to_string()))
}

#[derive(Debug)]
pub enum FragmentError {
    /// The file's pass1 output cannot be represented as a self-contained
    /// fragment (e.g. it references an ID outside its own window).
    NonCacheable(String),
    /// The fragment is corrupt or does not fit the current tables
    /// (e.g. a restored symbol conflicts with an existing one).
    Restore(String),
}

impl fmt::Display for FragmentError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FragmentError::NonCacheable(x) => write!(f, "non-cacheable fragment: {x}"),
            FragmentError::Restore(x) => write!(f, "fragment restore failed: {x}"),
        }
    }
}

impl std::error::Error for FragmentError {}

/// Captures everything the file's parse + pass1 (delimited by the
/// watermark) wrote into the global tables. Must be called before
/// `analyze_post_pass1`.
pub fn capture(
    src_path: &Path,
    source_text: &str,
    watermark: &FragmentWatermark,
) -> Result<Fragment, FragmentError> {
    let path_id = resource_table::insert_path(src_path);

    let token_end = resource_table::peek_token_id();
    let text_end = text_table::peek_text_id();
    let symbol_end = symbol::peek_symbol_id();
    let definition_end = definition_table::peek_definition_id();

    // One parse issues exactly one text ID; anything else means the
    // watermark does not delimit a single file.
    if text_end - watermark.text != 1 {
        return Err(FragmentError::NonCacheable(format!(
            "expected exactly one text id in window, got {}",
            text_end - watermark.text
        )));
    }

    let payload = FragmentPayload {
        doc_comments: doc_comment_table::export_by_path(path_id),
        symbols: symbol_table::export_fragment(watermark.symbol, symbol_end, &watermark.pending),
        namespace_entries: namespace_table::export_by_path(path_id),
        literals: literal_table::export_in_window(watermark.token, token_end),
        attributes: attribute_table::export_by_path(path_id),
        unsafes: unsafe_table::export_by_path(path_id),
        definitions: definition_table::export_in_window(watermark.definition, definition_end),
        reference_candidates: reference_table::export_candidates_since(
            watermark.reference_candidates,
        ),
        type_dag_candidates: type_dag::export_candidates_since(watermark.type_dag_candidates),
        generic_inference_pending: generic_inference_table::export_pending_since(
            watermark.generic_pending,
        ),
    };

    parser_codec::begin_encode(parser_codec::EncodeSession::new(
        IdWindow {
            start: watermark.token,
            end: token_end,
        },
        IdWindow {
            start: watermark.text,
            end: text_end,
        },
    ));
    fragment_codec::begin_encode(fragment_codec::EncodeSession {
        symbol_window: IdWindow {
            start: watermark.symbol,
            end: symbol_end,
        },
        definition_window: IdWindow {
            start: watermark.definition,
            end: definition_end,
        },
    });
    let bytes = postcard::to_allocvec(&payload);
    fragment_codec::end_encode();
    let dicts = parser_codec::end_encode().expect("encode session must be active");

    let payload = bytes.map_err(|x| FragmentError::NonCacheable(x.to_string()))?;

    Ok(Fragment {
        src_path: src_path.to_path_buf(),
        source_text: source_text.to_string(),
        strings: dicts.strings,
        paths: dicts.paths,
        token_count: token_end - watermark.token,
        symbol_count: symbol_end - watermark.symbol,
        definition_count: definition_end - watermark.definition,
        payload,
    })
}

/// Restores a fragment into the global tables, replacing parse +
/// `analyze_pass1` for the file. The caller must do the project setup pass1
/// would (`namespace_table::set_project`) and have run `Analyzer::new` at
/// least once (it inserts the shared project namespace symbol, which this
/// does not). On failure the tables may hold a partial restore; the caller
/// must `drop` the file and fall back to a regular parse + pass1.
pub fn restore(fragment: &Fragment) -> Result<(), FragmentError> {
    let path_id = resource_table::insert_path(&fragment.src_path);

    let token_base = resource_table::reserve_token_ids(fragment.token_count);
    let text_base = text_table::reserve_text_ids(1);
    let symbol_base = symbol::reserve_symbol_ids(fragment.symbol_count);
    let definition_base = definition_table::reserve_definition_ids(fragment.definition_count);

    // The window's single text ID (local 0) rebases to `text_base + 1`.
    text_table::insert_with_id(
        TextId(text_base + 1),
        TextInfo {
            text: fragment.source_text.clone(),
            path: path_id,
        },
    );

    parser_codec::begin_decode(parser_codec::DecodeSession::new(
        &fragment.strings,
        &fragment.paths,
        IdRebase {
            base: token_base,
            count: fragment.token_count,
        },
        IdRebase {
            base: text_base,
            count: 1,
        },
    ));
    fragment_codec::begin_decode(fragment_codec::DecodeSession {
        symbol_rebase: IdRebase {
            base: symbol_base,
            count: fragment.symbol_count,
        },
        definition_rebase: IdRebase {
            base: definition_base,
            count: fragment.definition_count,
        },
    });
    let payload: Result<FragmentPayload, _> = postcard::from_bytes(&fragment.payload);
    fragment_codec::end_decode();
    parser_codec::end_decode();

    let payload = payload.map_err(|x| FragmentError::Restore(x.to_string()))?;

    for (line, text) in payload.doc_comments {
        doc_comment_table::insert(path_id, line, text);
    }
    symbol_table::restore_fragment(payload.symbols)
        .map_err(|x| FragmentError::Restore(format!("symbol conflict on restore: {}", x.token)))?;
    for (id, namespace) in payload.namespace_entries {
        namespace_table::insert(id, path_id, &namespace);
    }
    for (id, literal) in payload.literals {
        literal_table::insert(id, literal);
    }
    for (range, attribute) in payload.attributes {
        attribute_table::insert(range, attribute);
    }
    for (range, value) in payload.unsafes {
        unsafe_table::insert(range, value);
    }
    for (id, definition) in payload.definitions {
        definition_table::insert_with_id(id, definition);
    }
    for candidate in payload.reference_candidates {
        reference_table::add(candidate);
    }
    for candidate in payload.type_dag_candidates {
        type_dag::add(candidate);
    }
    for entry in payload.generic_inference_pending {
        generic_inference_table::push_pending(entry);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Analyzer, AnalyzerError};
    use std::thread;
    use veryl_metadata::Metadata;
    use veryl_parser::Parser;

    const FILE_A: &str = r#"
    /// Package with shared types
    package PackageA {
        const WIDTH: u32 = 8;

        struct StructA {
            fieldA: logic<WIDTH>,
        }

        enum EnumA: logic<2> {
            memberA,
            memberB,
        }
    }
    "#;

    const FILE_B: &str = r#"
    import PackageA::*;

    /// Module covered by the fragment cache
    module ModuleB #(
        param P: u32 = WIDTH,
    ) (
        i_clk: input clock,
        i_rst: input reset,
        i_dat: input logic<P>,
        o_dat: output logic<P>,
    ) {
        var r: logic<P>;

        #[allow(unused_variable)]
        var unused: StructA;

        always_ff {
            if_reset {
                r = 0;
            } else {
                r = i_dat;
            }
        }

        assign o_dat = r;
    }
    "#;

    const FILE_C: &str = r#"
    module ModuleC (
        i_clk: input clock,
        i_rst: input reset,
        i_dat: input logic<8>,
        o_dat: output logic<8>,
    ) {
        inst u0: ModuleB #(P: 8) (
            i_clk,
            i_rst,
            i_dat,
            o_dat,
        );
    }
    "#;

    struct RunResult {
        symbol_dump: String,
        namespace_dump: String,
        type_dag_dump: String,
        file_dag_dump: String,
        errors: usize,
        fragment: Option<Fragment>,
    }

    /// Runs pass1 over the three files in a fresh thread (fresh
    /// thread-local tables and ID counters). When `restore_b` is given,
    /// b.veryl is restored from the fragment instead of being parsed.
    fn run(restore_b: Option<Fragment>) -> RunResult {
        let builder = thread::Builder::new().stack_size(16 * 1024 * 1024);
        let handler = builder
            .spawn(move || {
                let metadata = Metadata::create_default("prj").unwrap();
                let analyzer = Analyzer::new(&metadata);

                let files = [
                    ("a.veryl", FILE_A),
                    ("b.veryl", FILE_B),
                    ("c.veryl", FILE_C),
                ];

                let mut errors: Vec<AnalyzerError> = vec![];
                let mut fragment = None;
                let mut parsers = vec![];
                for (path, code) in files {
                    if path == "b.veryl"
                        && let Some(x) = &restore_b
                    {
                        restore(x).unwrap();
                        continue;
                    }
                    let wm = watermark();
                    let parser = Parser::parse(code, &path).unwrap();
                    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
                    if path == "b.veryl" {
                        fragment = Some(capture(Path::new(path), code, &wm).unwrap());
                    }
                    parsers.push(parser);
                }

                errors.append(&mut Analyzer::analyze_post_pass1());

                RunResult {
                    symbol_dump: symbol_table::dump(),
                    namespace_dump: namespace_table::dump(),
                    type_dag_dump: type_dag::dump(),
                    file_dag_dump: type_dag::dump_file(),
                    errors: errors.len(),
                    fragment,
                }
            })
            .unwrap();
        handler.join().unwrap()
    }

    #[test]
    fn restored_fragment_matches_direct_pass1() {
        let cold = run(None);
        assert_eq!(cold.errors, 0);
        let fragment = cold.fragment.as_ref().unwrap();
        assert!(fragment.symbol_count > 0);
        assert!(fragment.token_count > 0);

        // Serialize the whole fragment to bytes and back, as the disk
        // cache will.
        let bytes = postcard::to_allocvec(fragment).unwrap();
        let fragment: Fragment = postcard::from_bytes(&bytes).unwrap();

        let warm = run(Some(fragment));
        assert_eq!(warm.errors, 0);

        // Both runs start from fresh per-thread counters and allocate the
        // same ID ranges, so the dumps must match byte-for-byte.
        assert_eq!(cold.symbol_dump, warm.symbol_dump);
        assert_eq!(cold.namespace_dump, warm.namespace_dump);
        assert_eq!(cold.type_dag_dump, warm.type_dag_dump);
        assert_eq!(cold.file_dag_dump, warm.file_dag_dump);
    }
}
