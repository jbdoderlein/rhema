//! The seam through which a crate teaches the lifecycle how to read a
//! crash.

use std::collections::BTreeMap;
use std::sync::Arc;

use rhema_core::Session;
use serde::de::DeserializeOwned;

use crate::error::SubmissionError;
use crate::record::{KindName, Record};

/// What a crate that owns a kind of work must provide.
///
/// The kind knows only its own payload and its own store. The lifecycle
/// owns the state machine, the writes, and the durability rule, so a
/// kind never touches a record's status.
pub trait Kind: Send + Sync {
    /// The tag written into [`Record::kind`].
    fn name(&self) -> &'static str;

    /// What a `Running` record left by a dead process means now.
    ///
    /// Called once per stale record at load. Read-only: the kind may look
    /// at the session, another store, a daemon, but it returns a verdict
    /// and this crate does the write.
    fn recover(&self, ctx: &RecoverCtx<'_>) -> Recovery;
}

/// What a kind gets to read while it decides a stale record's fate.
pub struct RecoverCtx<'a> {
    /// The session the record was loaded with, as the dead process left
    /// it.
    pub session: &'a Session,
    /// The stale record itself.
    pub record: &'a Record,
}

impl RecoverCtx<'_> {
    /// The record's payload, typed by the kind that wrote it.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, SubmissionError> {
        serde_json::from_value(self.record.payload.clone()).map_err(|err| {
            SubmissionError::Payload {
                id: self.record.id.clone(),
                message: err.to_string(),
            }
        })
    }
}

/// What a dead process left behind. Exactly three answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// The work finished; the record is settled.
    Done,
    /// Nothing landed; safe to start again from the payload.
    Queued,
    /// Some of it landed; the driver knows how to continue.
    Interrupted,
}

/// The kinds a frontend links, by tag.
///
/// A build that does not link a kind still loads the sessions that
/// mention it: [`crate::reconcile`] reports those records as unclaimed
/// and never drops them.
#[derive(Default)]
pub struct Kinds {
    by_name: BTreeMap<KindName, Arc<dyn Kind>>,
}

impl Kinds {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `kind`, keyed by its own name.
    ///
    /// # Panics
    ///
    /// When the kind's name is not a valid tag, `[a-z0-9][a-z0-9-]{0,31}`.
    /// A kind that cannot name itself is a programming error, and failing
    /// at registration beats failing on the first record.
    pub fn with(mut self, kind: Arc<dyn Kind>) -> Self {
        let name = KindName::new(kind.name()).unwrap_or_else(|_| {
            panic!(
                "kind {name:?} is not a valid tag: the name must match [a-z0-9][a-z0-9-]{{0,31}}",
                name = kind.name()
            )
        });
        self.by_name.insert(name, kind);
        self
    }

    /// The kind registered under `name`, if this build links it.
    pub fn get(&self, name: &KindName) -> Option<&Arc<dyn Kind>> {
        self.by_name.get(name)
    }

    /// How many kinds are registered.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether no kind is registered.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

impl std::fmt::Debug for Kinds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.by_name.keys()).finish()
    }
}
