//! The validated session id: a file name component and nothing more.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::StoreError;

/// The longest id a store accepts, in bytes.
pub const MAX_ID_LEN: usize = 128;

/// An opaque, validated identifier for one saved session.
///
/// A `SessionId` is safe to join to a path: construction admits 1 to 128
/// bytes, every byte inside `[-._0-9A-Za-z]`, and no leading `.`. The dot
/// rule keeps a temp file from ever naming a session: `list` skips dot
/// entries, so an id with one would save but never list. No separator,
/// no `..`, no whitespace, and no percent-encoded separator fits the
/// allowed alphabet.
///
/// The TUI allocates `<millis:016>-<n:02>`; ACP uses the client's session
/// id; a path store has no id at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Validates `raw` and wraps it.
    pub fn new(raw: impl Into<String>) -> Result<Self, StoreError> {
        let raw = raw.into();
        if raw.is_empty()
            || raw.len() > MAX_ID_LEN
            || raw.starts_with('.')
            || !raw.bytes().all(is_file_safe)
        {
            return Err(StoreError::InvalidId(raw));
        }
        Ok(Self(raw))
    }

    /// The id as the file name component it is.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The one-byte alphabet an id may use; the same set the workspace
/// directory encoding leaves unescaped.
fn is_file_safe(byte: u8) -> bool {
    matches!(byte, b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z')
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

struct IdVisitor;

impl Visitor<'_> for IdVisitor {
    type Value = SessionId;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a valid session id")
    }

    fn visit_str<E: de::Error>(self, raw: &str) -> Result<Self::Value, E> {
        SessionId::new(raw).map_err(|err| E::custom(err.to_string()))
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(IdVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allocated_shapes_are_valid() {
        for id in ["0001759000000000-00", "rhema-abc-001", "x-y_z.9", "A"] {
            assert_eq!(SessionId::new(id).unwrap().as_str(), id);
        }
    }

    #[test]
    fn traversal_and_separators_are_rejected() {
        for raw in ["", "..", "../escape", "a/b", "a\\b", ".hidden", "a b", "é"] {
            assert!(
                matches!(SessionId::new(raw), Err(StoreError::InvalidId(_))),
                "{raw:?} must be rejected"
            );
        }
    }

    #[test]
    fn the_length_bounds_hold() {
        assert!(SessionId::new("a".repeat(MAX_ID_LEN)).is_ok());
        assert!(SessionId::new("a".repeat(MAX_ID_LEN + 1)).is_err());
    }

    #[test]
    fn deserialization_validates() {
        let ok: SessionId = serde_json::from_str("\"abc-1\"").unwrap();
        assert_eq!(ok.as_str(), "abc-1");
        assert!(serde_json::from_str::<SessionId>("\"../escape\"").is_err());
        assert!(serde_json::from_str::<SessionId>(".hidden").is_err());
    }

    #[test]
    fn serialization_is_the_bare_string() {
        let id = SessionId::new("abc-1").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"abc-1\"");
        assert_eq!(id.to_string(), "abc-1");
        assert_eq!(id.as_ref(), "abc-1");
    }

    #[test]
    fn ids_order_lexicographically() {
        let mut ids = [
            SessionId::new("0001759000000000-01").unwrap(),
            SessionId::new("0001759000000000-00").unwrap(),
        ];
        ids.sort();
        assert_eq!(ids[0].as_str(), "0001759000000000-00");
    }
}
