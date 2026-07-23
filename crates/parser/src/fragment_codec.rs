//! ID remapping for serialized analyzer fragments.
//!
//! Global IDs aren't reproducible across runs, so fragments store local
//! ones: counter IDs (`TokenId`/`TextId`) as per-file window offsets
//! (rebased on decode), interned IDs (`StrId`/`PathId`) as dictionary
//! indices (re-interned on decode). The ID types' custom serde consults a
//! thread-local session; inactive means passthrough, and an out-of-window
//! ID errors out so the fragment is treated as non-cacheable.

use crate::resource_table::{self, PathId, StrId, TokenId};
use crate::text_table::TextId;
use serde::de::Error as DeError;
use serde::ser::Error as SerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

/// Half-open ID window `(start, end]` matching the pre/post counter values
/// around a single file's parse + pass1.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdWindow {
    pub start: usize,
    pub end: usize,
}

impl IdWindow {
    pub fn count(&self) -> usize {
        self.end - self.start
    }

    // Analyzer-side ids layer a sentinel-0 wire shift on top of this (see
    // veryl_analyzer::fragment_codec); parser ids have no 0 sentinel.
    pub fn encode(&self, id: usize, what: &str) -> Result<u64, String> {
        if id > self.start && id <= self.end {
            Ok((id - self.start - 1) as u64)
        } else {
            Err(format!(
                "fragment codec: {what} {id} outside window ({}, {}]",
                self.start, self.end
            ))
        }
    }
}

/// Rebases decoded local IDs onto a freshly reserved counter range.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdRebase {
    pub base: usize,
    pub count: usize,
}

impl IdRebase {
    pub fn decode(&self, local: u64, what: &str) -> Result<usize, String> {
        let local = local as usize;
        if local < self.count {
            Ok(self.base + local + 1)
        } else {
            Err(format!(
                "fragment codec: {what} local id {local} out of range (count {})",
                self.count
            ))
        }
    }
}

#[derive(Default)]
pub struct EncodeSession {
    str_map: HashMap<StrId, u32>,
    str_dict: Vec<String>,
    path_map: HashMap<PathId, u32>,
    path_dict: Vec<PathBuf>,
    token_window: IdWindow,
    text_window: IdWindow,
}

impl EncodeSession {
    pub fn new(token_window: IdWindow, text_window: IdWindow) -> Self {
        Self {
            token_window,
            text_window,
            ..Default::default()
        }
    }

    fn encode_str(&mut self, id: StrId) -> Result<u64, String> {
        if let Some(x) = self.str_map.get(&id) {
            return Ok(*x as u64);
        }
        let value = resource_table::get_str_value(id)
            .ok_or_else(|| format!("fragment codec: unknown StrId {}", id.0))?;
        let local = self.str_dict.len() as u32;
        self.str_dict.push(value);
        self.str_map.insert(id, local);
        Ok(local as u64)
    }

    fn encode_path(&mut self, id: PathId) -> Result<u64, String> {
        if let Some(x) = self.path_map.get(&id) {
            return Ok(*x as u64);
        }
        let value = resource_table::get_path_value(id)
            .ok_or_else(|| format!("fragment codec: unknown PathId {}", id.0))?;
        let local = self.path_dict.len() as u32;
        self.path_dict.push(value);
        self.path_map.insert(id, local);
        Ok(local as u64)
    }
}

/// Dictionaries produced by an encode session. Stored alongside the encoded
/// payload and used to seed the decode session.
pub struct EncodeDicts {
    pub strings: Vec<String>,
    pub paths: Vec<PathBuf>,
}

pub struct DecodeSession {
    strs: Vec<StrId>,
    paths: Vec<PathId>,
    token_rebase: IdRebase,
    text_rebase: IdRebase,
}

impl DecodeSession {
    /// Re-interns the dictionaries into the live tables and prepares rebases.
    /// The caller must have reserved `token_rebase`/`text_rebase` ranges.
    pub fn new(
        strings: &[String],
        paths: &[PathBuf],
        token_rebase: IdRebase,
        text_rebase: IdRebase,
    ) -> Self {
        let strs = strings
            .iter()
            .map(|x| resource_table::insert_str(x))
            .collect();
        let paths = paths
            .iter()
            .map(|x| resource_table::insert_path(x))
            .collect();
        Self {
            strs,
            paths,
            token_rebase,
            text_rebase,
        }
    }

    fn decode_str(&self, local: u64) -> Result<StrId, String> {
        self.strs
            .get(local as usize)
            .copied()
            .ok_or_else(|| format!("fragment codec: StrId index {local} out of dictionary"))
    }

    fn decode_path(&self, local: u64) -> Result<PathId, String> {
        self.paths
            .get(local as usize)
            .copied()
            .ok_or_else(|| format!("fragment codec: PathId index {local} out of dictionary"))
    }
}

thread_local!(static ENCODE: RefCell<Option<EncodeSession>> = const { RefCell::new(None) });
thread_local!(static DECODE: RefCell<Option<DecodeSession>> = const { RefCell::new(None) });

pub fn begin_encode(session: EncodeSession) {
    ENCODE.with(|f| *f.borrow_mut() = Some(session));
}

/// Ends the encode session and returns the collected dictionaries.
pub fn end_encode() -> Option<EncodeDicts> {
    ENCODE.with(|f| {
        f.borrow_mut().take().map(|x| EncodeDicts {
            strings: x.str_dict,
            paths: x.path_dict,
        })
    })
}

pub fn begin_decode(session: DecodeSession) {
    DECODE.with(|f| *f.borrow_mut() = Some(session));
}

pub fn end_decode() {
    DECODE.with(|f| *f.borrow_mut() = None);
}

fn with_encode<R>(f: impl FnOnce(Option<&mut EncodeSession>) -> R) -> R {
    ENCODE.with(|x| f(x.borrow_mut().as_mut()))
}

fn with_decode<R>(f: impl FnOnce(Option<&DecodeSession>) -> R) -> R {
    DECODE.with(|x| f(x.borrow().as_ref()))
}

impl Serialize for StrId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = with_encode(|session| match session {
            Some(session) => session.encode_str(*self),
            None => Ok(self.0 as u64),
        })
        .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for StrId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        with_decode(|session| match session {
            Some(session) => session.decode_str(value),
            None => Ok(StrId(value as usize)),
        })
        .map_err(D::Error::custom)
    }
}

impl Serialize for PathId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = with_encode(|session| match session {
            Some(session) => session.encode_path(*self),
            None => Ok(self.0 as u64),
        })
        .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for PathId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        with_decode(|session| match session {
            Some(session) => session.decode_path(value),
            None => Ok(PathId(value as usize)),
        })
        .map_err(D::Error::custom)
    }
}

impl Serialize for TokenId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = with_encode(|session| match session {
            Some(session) => session.token_window.encode(self.0, "TokenId"),
            None => Ok(self.0 as u64),
        })
        .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for TokenId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        with_decode(|session| match session {
            Some(session) => session.token_rebase.decode(value, "TokenId").map(TokenId),
            None => Ok(TokenId(value as usize)),
        })
        .map_err(D::Error::custom)
    }
}

impl Serialize for TextId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = with_encode(|session| match session {
            Some(session) => session.text_window.encode(self.0, "TextId"),
            None => Ok(self.0 as u64),
        })
        .map_err(S::Error::custom)?;
        serializer.serialize_u64(value)
    }
}

impl<'de> Deserialize<'de> for TextId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        with_decode(|session| match session {
            Some(session) => session.text_rebase.decode(value, "TextId").map(TextId),
            None => Ok(TextId(value as usize)),
        })
        .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_table::{self, TextInfo};
    use crate::veryl_token::{Token, TokenSource};
    use std::path::Path;

    #[test]
    fn token_roundtrip_rebases_ids() {
        let token_start = resource_table::peek_token_id();
        let text_start = text_table::peek_text_id();

        let path = resource_table::insert_path(Path::new("roundtrip.veryl"));
        let text = text_table::set_current_text(TextInfo {
            text: "module A {}".to_string(),
            path,
        });
        let source = TokenSource::File { path, text };
        let a = Token::new("alpha", 1, 2, 5, 0, source);
        let b = Token::new("beta", 2, 3, 4, 10, source);

        let token_end = resource_table::peek_token_id();
        let text_end = text_table::peek_text_id();
        let token_count = token_end - token_start;
        let text_count = text_end - text_start;

        begin_encode(EncodeSession::new(
            IdWindow {
                start: token_start,
                end: token_end,
            },
            IdWindow {
                start: text_start,
                end: text_end,
            },
        ));
        let bytes = postcard::to_allocvec(&(a, b)).unwrap();
        let dicts = end_encode().unwrap();
        assert!(dicts.strings.contains(&"alpha".to_string()));
        assert!(
            dicts
                .paths
                .contains(&Path::new("roundtrip.veryl").to_path_buf())
        );

        let token_base = resource_table::reserve_token_ids(token_count);
        let text_base = text_table::reserve_text_ids(text_count);
        begin_decode(DecodeSession::new(
            &dicts.strings,
            &dicts.paths,
            IdRebase {
                base: token_base,
                count: token_count,
            },
            IdRebase {
                base: text_base,
                count: text_count,
            },
        ));
        let (a2, b2): (Token, Token) = postcard::from_bytes(&bytes).unwrap();
        end_decode();

        // IDs are rebased onto the reserved range, preserving relative order
        assert_eq!(a2.id.0, token_base + (a.id.0 - token_start));
        assert_eq!(b2.id.0, token_base + (b.id.0 - token_start));
        assert!(a2.id < b2.id);
        // interned values survive the roundtrip
        assert_eq!(a2.text, resource_table::insert_str("alpha"));
        assert_eq!(b2.text, resource_table::insert_str("beta"));
        assert_eq!((a2.line, a2.column, a2.length, a2.pos), (1, 2, 5, 0));
        match a2.source {
            TokenSource::File { path, text } => {
                assert_eq!(
                    resource_table::get_path_value(path).unwrap(),
                    Path::new("roundtrip.veryl").to_path_buf()
                );
                assert_eq!(text.0, text_base + 1);
            }
            _ => panic!("source kind changed"),
        }
    }

    #[test]
    fn out_of_window_token_fails_encode() {
        let outside = Token::new("x", 0, 0, 1, 0, TokenSource::External);
        let start = resource_table::peek_token_id();
        let _inside = Token::new("y", 0, 0, 1, 0, TokenSource::External);
        let end = resource_table::peek_token_id();

        begin_encode(EncodeSession::new(
            IdWindow { start, end },
            IdWindow::default(),
        ));
        let result = postcard::to_allocvec(&outside);
        end_encode();
        assert!(result.is_err());
    }

    #[test]
    fn passthrough_without_session() {
        let bytes = postcard::to_allocvec(&TokenId(42)).unwrap();
        let id: TokenId = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(id, TokenId(42));
    }
}
