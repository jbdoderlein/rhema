//! Why a submission operation failed.

use rhema_core::CommitError;
use rhema_session_store::SessionId;

use crate::record::Status;

/// Every failure the lifecycle reports.
///
/// The patterns are stable: a driver that wants to react reads the
/// variant, never the rendered text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmissionError {
    /// The ledger's `v` is not this build's: a newer harness wrote a
    /// shape this one would corrupt, so the driver must not start work.
    #[error("submission ledger version {0} is not supported by this build")]
    Unsupported(u32),
    /// The ledger does not parse: an unknown status, an invalid id or
    /// kind tag, a field of the wrong type. The session is untouched.
    #[error("submission ledger is corrupt: {0}")]
    Corrupt(String),
    /// No record carries this id.
    #[error("no submission {0}")]
    NotFound(SessionId),
    /// The state machine does not list this transition.
    #[error("submission {id} cannot go from {from} to {to}")]
    Transition {
        /// The record that refused the transition.
        id: SessionId,
        /// The status it was in.
        from: Status,
        /// The status the caller asked for.
        to: Status,
    },
    /// The failure was classed permanent at settle time, so `retry` may
    /// not start the record again.
    #[error("submission {id} failed in a way a retry cannot fix")]
    NotRetryable {
        /// The record the caller tried to retry.
        id: SessionId,
    },
    /// The record's payload is not the kind's schema.
    #[error("submission {id}'s payload is not the kind's schema: {message}")]
    Payload {
        /// The record whose payload failed to parse.
        id: SessionId,
        /// What the parser said.
        message: String,
    },
    /// A kind tag outside the grammar, `[a-z0-9][a-z0-9-]{0,31}`.
    #[error("invalid kind tag {0:?}")]
    InvalidKind(String),
    /// The caller's own save failed, so the transition is not durable.
    #[error(transparent)]
    Commit(#[from] CommitError),
}
