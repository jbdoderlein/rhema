//! The listing row: metadata only, never the session body.

use crate::id::SessionId;

/// One saved session as a picker needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The saved record's id.
    pub id: SessionId,
    /// When it was saved, in milliseconds since the Unix epoch (the
    /// file's mtime, or the save clock for SQLite).
    pub saved_at: u64,
    /// The record's byte length.
    pub bytes: u64,
    /// The first non-empty user text, cut to
    /// [`OPENING_PROMPT_WIDTH`](crate::OPENING_PROMPT_WIDTH) characters;
    /// `None` when the record has no user text or its body does not
    /// parse.
    pub opening_prompt: Option<String>,
}
