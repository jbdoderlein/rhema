//! `rhema-submissions`, the lifecycle layer for requested work.
//!
//! Every requested unit of work has a durable id, a kind, a payload and a
//! status. A resumed process can always tell which requests finished,
//! which are half done, and which never started, and the crate that owns
//! a kind decides what "finished" means for it.
//!
//! The records live in [`rhema_core::Session::driver_state`], under the
//! `submissions` key: the store already writes the whole session, so the
//! work's state and its status travel in one record on every backend.
//! This crate knows no backend, and saves nothing: every mutator writes
//! the session and returns, and the caller's `store.save` makes the
//! transition durable.
//!
//! The kinds are open. This crate ships one, [`turn`], because
//! `rhema_core::Agent`'s turn is the engine's unit of work. A subagent,
//! a job, or a question implements [`Kind`] in the crate that owns the
//! work and registers it in a [`Kinds`]; nothing in this crate changes
//! when one lands.
//!
//! Layout:
//! - [`record`]: `Record`, `Status`, `Outcome`, `Owner`, `KindName`
//! - [`state`]: `submit`, `start`, `settle`, `cancel`, `retry`,
//!   `reconcile`, and the ledger in `driver_state`
//! - [`view`]: the read-only `Snapshot`
//! - [`kind`]: `Kind`, `RecoverCtx`, `Recovery`, `Kinds`, the unknown-kind
//!   rule
//! - [`turn`]: the built-in turn kind, its `drive`, `resume`, and `answer`
//! - [`error`] carries `SubmissionError`

#![forbid(unsafe_code)]

pub mod error;
pub mod kind;
pub mod record;
pub mod state;
pub mod turn;
pub mod view;

pub use error::SubmissionError;
pub use kind::{Kind, Kinds, RecoverCtx, Recovery};
pub use record::{KindName, MAX_KIND_NAME, Millis, Outcome, Owner, Record, Status, SubmissionId};
pub use state::{
    Cancel, Change, RETAIN_SETTLED, cancel, next_id, reconcile, retry, settle, start, submit,
};
pub use view::{Snapshot, snapshot};

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time in milliseconds since the epoch.
///
/// The lifecycle takes [`Millis`] from its caller so tests pass fixed
/// values; the built-in turn kind's `drive` and `resume` stamp their own
/// writes with this.
pub fn now() -> Millis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}
