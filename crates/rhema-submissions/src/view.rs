//! The read-only view over a session's ledger.

use rhema_core::Session;

use crate::error::SubmissionError;
use crate::record::{Record, Status, SubmissionId};
use crate::state::Ledger;

/// A parsed, owned view of the ledger, oldest first by id.
///
/// Reading a snapshot never mutates the session, so a frontend can list
/// what a session asked for without holding a `&mut Session`.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    records: Vec<Record>,
}

impl Snapshot {
    pub(crate) fn from_records(records: Vec<Record>) -> Self {
        Self { records }
    }

    /// The record with this id, if the ledger has one.
    pub fn get(&self, id: &SubmissionId) -> Option<&Record> {
        self.records.iter().find(|record| &record.id == id)
    }

    /// The oldest `Queued` record: what a serial driver starts next.
    ///
    /// Running records do not appear here, because several kinds can run
    /// at once and each driver owns the ones it started.
    pub fn active(&self) -> Option<&Record> {
        self.records
            .iter()
            .find(|record| record.status == Status::Queued)
    }

    /// The newest record, settled or not.
    pub fn newest(&self) -> Option<&Record> {
        self.records.last()
    }

    /// Every record, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Record> {
        self.records.iter()
    }

    /// How many records the ledger holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// A read-only view of the session's ledger.
///
/// A missing `submissions` key, a `null` driver state, or one holding
/// other keys all read as an empty ledger.
pub fn snapshot(session: &Session) -> Result<Snapshot, SubmissionError> {
    Ok(Snapshot::from_records(
        Ledger::parse(&session.driver_state)?.records(),
    ))
}
