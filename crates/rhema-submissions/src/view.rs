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

    /// The queued or running record a driver should pick up, if any.
    ///
    /// v1 runs one submission at a time, in submission order, so this is
    /// the oldest record that is `Queued` or `Running`.
    pub fn active(&self) -> Option<&Record> {
        self.records
            .iter()
            .find(|record| matches!(record.status, Status::Queued | Status::Running))
    }

    /// The newest record, settled or not.
    pub fn newest(&self) -> Option<&Record> {
        self.records.last()
    }

    /// Every record, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Record> {
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
