//! Analyzer-side fragment codec: ID remapping for `SymbolId`/`DefinitionId`.
//!
//! Mirrors [`veryl_parser::fragment_codec`] (window/rebase scheme); both
//! crates' sessions are driven together by the capture/restore code.

use crate::definition_table::DefinitionId;
use crate::symbol::SymbolId;
use serde::de::Error as DeError;
use serde::ser::Error as SerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use veryl_parser::fragment_codec::{IdRebase, IdWindow};

#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeSession {
    pub symbol_window: IdWindow,
    pub definition_window: IdWindow,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DecodeSession {
    pub symbol_rebase: IdRebase,
    pub definition_rebase: IdRebase,
}

thread_local!(static ENCODE: RefCell<Option<EncodeSession>> = const { RefCell::new(None) });
thread_local!(static DECODE: RefCell<Option<DecodeSession>> = const { RefCell::new(None) });

pub fn begin_encode(session: EncodeSession) {
    ENCODE.with(|f| *f.borrow_mut() = Some(session));
}

pub fn end_encode() {
    ENCODE.with(|f| *f.borrow_mut() = None);
}

pub fn begin_decode(session: DecodeSession) {
    DECODE.with(|f| *f.borrow_mut() = Some(session));
}

pub fn end_decode() {
    DECODE.with(|f| *f.borrow_mut() = None);
}

// Id 0 is the unresolved-reference sentinel a later pass fills in (e.g. a
// modport member); real ids start at 1, so wire value 0 is free for it.
fn encode_sentinel(window: &IdWindow, id: usize, what: &str) -> Result<u64, String> {
    if id == 0 {
        Ok(0)
    } else {
        window.encode(id, what).map(|v| v + 1)
    }
}

fn decode_sentinel(rebase: &IdRebase, value: u64, what: &str) -> Result<usize, String> {
    if value == 0 {
        Ok(0)
    } else {
        rebase.decode(value - 1, what)
    }
}

impl Serialize for SymbolId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = ENCODE
            .with(|x| match x.borrow().as_ref() {
                Some(session) => encode_sentinel(&session.symbol_window, self.0, "SymbolId"),
                None => Ok(self.0 as u64),
            })
            .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for SymbolId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        DECODE
            .with(|x| match x.borrow().as_ref() {
                Some(session) => {
                    decode_sentinel(&session.symbol_rebase, value, "SymbolId").map(SymbolId)
                }
                None => Ok(SymbolId(value as usize)),
            })
            .map_err(D::Error::custom)
    }
}

impl Serialize for DefinitionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = ENCODE
            .with(|x| match x.borrow().as_ref() {
                Some(session) => {
                    encode_sentinel(&session.definition_window, self.0, "DefinitionId")
                }
                None => Ok(self.0 as u64),
            })
            .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for DefinitionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        DECODE
            .with(|x| match x.borrow().as_ref() {
                Some(session) => decode_sentinel(&session.definition_rebase, value, "DefinitionId")
                    .map(DefinitionId),
                None => Ok(DefinitionId(value as usize)),
            })
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::Namespace;
    use crate::symbol::{self, DocComment, Symbol, SymbolKind};
    use veryl_parser::fragment_codec as parser_codec;
    use veryl_parser::resource_table;
    use veryl_parser::veryl_token::{Token, TokenSource};

    // Runs the capture/restore protocol (the parser session must bracket the
    // analyzer session) over a symbol built after the given watermarks;
    // returns (restored symbol, token base, symbol base).
    fn roundtrip(sym: &Symbol, token_start: usize, symbol_start: usize) -> (Symbol, usize, usize) {
        let token_end = resource_table::peek_token_id();
        let symbol_end = symbol::peek_symbol_id();
        let token_count = token_end - token_start;
        let symbol_count = symbol_end - symbol_start;

        parser_codec::begin_encode(parser_codec::EncodeSession::new(
            IdWindow {
                start: token_start,
                end: token_end,
            },
            IdWindow::default(),
        ));
        begin_encode(EncodeSession {
            symbol_window: IdWindow {
                start: symbol_start,
                end: symbol_end,
            },
            definition_window: IdWindow::default(),
        });
        let bytes = postcard::to_allocvec(sym).expect("symbol must encode");
        end_encode();
        let dicts = parser_codec::end_encode().unwrap();

        let token_base = resource_table::reserve_token_ids(token_count);
        let symbol_base = symbol::reserve_symbol_ids(symbol_count);
        parser_codec::begin_decode(parser_codec::DecodeSession::new(
            &dicts.strings,
            &dicts.paths,
            IdRebase {
                base: token_base,
                count: token_count,
            },
            IdRebase::default(),
        ));
        begin_decode(DecodeSession {
            symbol_rebase: IdRebase {
                base: symbol_base,
                count: symbol_count,
            },
            definition_rebase: IdRebase::default(),
        });
        let sym2: Symbol = postcard::from_bytes(&bytes).unwrap();
        end_decode();
        parser_codec::end_decode();

        (sym2, token_base, symbol_base)
    }

    #[test]
    fn symbol_roundtrip_rebases_ids() {
        let token_start = resource_table::peek_token_id();
        let symbol_start = symbol::peek_symbol_id();

        let token = Token::new("foo", 1, 1, 3, 0, TokenSource::External);
        let mut namespace = Namespace::new();
        namespace.push(resource_table::insert_str("prj"));
        let sym = Symbol::new(
            &token,
            SymbolKind::Namespace,
            &namespace,
            true,
            DocComment::default(),
        );

        let (sym2, token_base, symbol_base) = roundtrip(&sym, token_start, symbol_start);

        assert_eq!(sym2.id.0, symbol_base + (sym.id.0 - symbol_start));
        assert_eq!(sym2.token.id.0, token_base + (sym.token.id.0 - token_start));
        assert_eq!(sym2.token.text, resource_table::insert_str("foo"));
        assert!(matches!(sym2.kind, SymbolKind::Namespace));
        assert_eq!(sym2.namespace, namespace);
        assert!(sym2.public);
    }

    #[test]
    fn out_of_window_symbol_fails_encode() {
        let token = Token::new("bar", 1, 1, 3, 0, TokenSource::External);
        let outside = Symbol::new(
            &token,
            SymbolKind::Namespace,
            &Namespace::new(),
            false,
            DocComment::default(),
        );
        let symbol_start = symbol::peek_symbol_id();

        parser_codec::begin_encode(parser_codec::EncodeSession::new(
            IdWindow {
                start: 0,
                end: usize::MAX,
            },
            IdWindow::default(),
        ));
        begin_encode(EncodeSession {
            symbol_window: IdWindow {
                start: symbol_start,
                end: symbol_start,
            },
            definition_window: IdWindow::default(),
        });
        let result = postcard::to_allocvec(&outside);
        end_encode();
        parser_codec::end_encode();
        assert!(result.is_err());
    }

    // The unresolved-reference sentinel id 0 must round-trip as 0, not be
    // rejected as out-of-window.
    #[test]
    fn sentinel_id_zero_roundtrips() {
        use crate::symbol::{Direction, ModportVariableMemberProperty};

        let token_start = resource_table::peek_token_id();
        let symbol_start = symbol::peek_symbol_id();

        let token = Token::new("mp_member", 1, 1, 3, 0, TokenSource::External);
        let sym = Symbol::new(
            &token,
            SymbolKind::ModportVariableMember(ModportVariableMemberProperty {
                direction: Direction::Input,
                variable: SymbolId(0),
            }),
            &Namespace::new(),
            false,
            DocComment::default(),
        );

        let (sym2, _, symbol_base) = roundtrip(&sym, token_start, symbol_start);

        // The symbol's own id rebases; the sentinel reference stays 0.
        assert_eq!(sym2.id.0, symbol_base + (sym.id.0 - symbol_start));
        let SymbolKind::ModportVariableMember(x) = &sym2.kind else {
            panic!("kind changed");
        };
        assert_eq!(x.variable, SymbolId(0));
    }

    #[test]
    fn definition_id_sentinel_and_shift_roundtrip() {
        // Sentinel round-trips even with empty windows.
        begin_encode(EncodeSession::default());
        let bytes = postcard::to_allocvec(&DefinitionId(0)).unwrap();
        end_encode();
        begin_decode(DecodeSession::default());
        let id: DefinitionId = postcard::from_bytes(&bytes).unwrap();
        end_decode();
        assert_eq!(id, DefinitionId(0));

        // Real id: the encode/decode shifts must stay paired.
        begin_encode(EncodeSession {
            symbol_window: IdWindow::default(),
            definition_window: IdWindow { start: 10, end: 20 },
        });
        let bytes = postcard::to_allocvec(&DefinitionId(15)).unwrap();
        end_encode();
        begin_decode(DecodeSession {
            symbol_rebase: IdRebase::default(),
            definition_rebase: IdRebase {
                base: 100,
                count: 10,
            },
        });
        let id: DefinitionId = postcard::from_bytes(&bytes).unwrap();
        end_decode();
        // Window-local 4 rebased onto base 100.
        assert_eq!(id, DefinitionId(105));
    }
}
