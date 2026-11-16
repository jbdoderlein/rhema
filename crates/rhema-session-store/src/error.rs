//! The single error type every store method returns.

use std::io;
use std::path::PathBuf;

use crate::id::SessionId;

/// Why a store operation failed.
///
/// A missing record is never one of these: `load` answers `Ok(None)`,
/// `delete` `Ok(false)`, and `list` an empty vector. Every failure the
/// backend cannot classify as absence or corruption arrives as
/// [`StoreError::Io`] or [`StoreError::Backend`]; no method panics on
/// input, paths, or a corrupt file.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A filesystem operation failed at `path`.
    #[error("session store io at {path}: {source}")]
    Io {
        /// The file or directory the operation named.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// A record was found but its body does not parse as a session.
    /// `list` tolerates this (the row lists without a prompt); `load`
    /// reports it.
    #[error("session {id} does not parse: {source}")]
    Corrupt {
        /// The record that failed to parse.
        id: SessionId,
        /// The parser's error.
        #[source]
        source: serde_json::Error,
    },
    /// A raw string is not a valid [`SessionId`].
    #[error("invalid session id {0:?}")]
    InvalidId(String),
    /// The backend failed for a reason of its own (a SQLite statement
    /// rejected the row, a session that cannot be serialized).
    #[error("session store backend: {0}")]
    Backend(String),
}

impl From<StoreError> for rhema_core::CommitError {
    fn from(err: StoreError) -> Self {
        rhema_core::CommitError(err.to_string())
    }
}
