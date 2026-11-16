//! The lifecycle: submit, start, settle, cancel, retry, reconcile.
//!
//! Every mutator parses the ledger out of `Session::driver_state`, applies
//! one transition, writes it back, and returns before any save. The caller
//! owns durability: `store.save` after each successful call, and it must
//! not proceed when that save fails, because the transition did not become
//! durable.

use std::collections::BTreeMap;

use rhema_core::Session;
use serde::{Deserialize, Serialize};

use crate::error::SubmissionError;
use crate::kind::{AdoptCtx, Kinds, RecoverCtx, Recovery};
use crate::record::{KindName, Millis, Outcome, Owner, Record, Status, SubmissionId};
use crate::view::Snapshot;

/// The `driver_state` key the ledger lives under.
pub(crate) const KEY: &str = "submissions";

/// The only ledger version this build reads and writes.
const VERSION: u32 = 1;

/// How many settled records the ledger keeps. Every unsettled record is
/// kept regardless, and the oldest settled ones go first.
///
/// The cap is a constant, not configuration: the ledger rides every
/// commit, so it must not grow with the session's life.
pub const RETAIN_SETTLED: usize = 32;

/// One record as the ledger stores it: the id is the map key, not a
/// field.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRecord {
    kind: KindName,
    payload: serde_json::Value,
    status: Status,
    created_at: Millis,
    #[serde(default)]
    started_at: Option<Millis>,
    #[serde(default)]
    settled_at: Option<Millis>,
    #[serde(default)]
    owner: Option<Owner>,
    #[serde(default)]
    outcome: Option<Outcome>,
    #[serde(default)]
    attempts: u32,
}

impl StoredRecord {
    fn into_record(self, id: SubmissionId) -> Record {
        Record {
            id,
            kind: self.kind,
            payload: self.payload,
            status: self.status,
            created_at: self.created_at,
            started_at: self.started_at,
            settled_at: self.settled_at,
            owner: self.owner,
            outcome: self.outcome,
            attempts: self.attempts,
        }
    }
}

impl From<&Record> for StoredRecord {
    fn from(record: &Record) -> Self {
        Self {
            kind: record.kind.clone(),
            payload: record.payload.clone(),
            status: record.status,
            created_at: record.created_at,
            started_at: record.started_at,
            settled_at: record.settled_at,
            owner: record.owner,
            outcome: record.outcome.clone(),
            attempts: record.attempts,
        }
    }
}

/// The whole ledger as it sits in `driver_state.submissions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Ledger {
    v: u32,
    #[serde(default)]
    items: BTreeMap<SubmissionId, StoredRecord>,
}

impl Ledger {
    /// Parses the ledger out of a session's driver state.
    ///
    /// A missing key, a `null` driver state, or a driver state holding
    /// other keys all read as an empty ledger; a ledger from a newer
    /// build is [`SubmissionError::Unsupported`]; a ledger that does not
    /// parse is [`SubmissionError::Corrupt`].
    pub(crate) fn parse(state: &serde_json::Value) -> Result<Self, SubmissionError> {
        let Some(raw) = state.get(KEY) else {
            return Ok(Self::empty());
        };
        if raw.is_null() {
            return Ok(Self::empty());
        }
        let stored: Self = serde_json::from_value(raw.clone())
            .map_err(|err| SubmissionError::Corrupt(err.to_string()))?;
        if stored.v != VERSION {
            return Err(SubmissionError::Unsupported(stored.v));
        }
        Ok(stored)
    }

    fn empty() -> Self {
        Self {
            v: VERSION,
            items: BTreeMap::new(),
        }
    }

    /// The records, oldest first by id.
    pub(crate) fn records(&self) -> Vec<Record> {
        self.items
            .iter()
            .map(|(id, stored)| stored.clone().into_record(id.clone()))
            .collect()
    }

    /// Drops the oldest settled records past [`RETAIN_SETTLED`], keeping
    /// every unsettled one.
    fn prune(&mut self) {
        let settled: Vec<SubmissionId> = self
            .items
            .iter()
            .filter(|(_, stored)| stored.status.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        let dropped = self.settled_over_cap();
        for id in settled.into_iter().take(dropped) {
            self.items.remove(&id);
        }
    }

    fn settled_over_cap(&self) -> usize {
        self.items
            .values()
            .filter(|stored| stored.status.is_terminal())
            .count()
            .saturating_sub(RETAIN_SETTLED)
    }

    /// Writes the ledger back into the session's driver state, keeping
    /// every other key there.
    fn store(self, session: &mut Session) -> Result<(), SubmissionError> {
        let state = &mut session.driver_state;
        if state.is_null() {
            *state = serde_json::Value::Object(serde_json::Map::new());
        }
        let Some(object) = state.as_object_mut() else {
            return Err(SubmissionError::Corrupt(
                "driver_state is not an object".to_owned(),
            ));
        };
        let value =
            serde_json::to_value(&self).map_err(|err| SubmissionError::Corrupt(err.to_string()))?;
        object.insert(KEY.to_owned(), value);
        Ok(())
    }

    /// Prunes and writes, the tail every mutation shares.
    fn finish(mut self, session: &mut Session) -> Result<(), SubmissionError> {
        self.prune();
        self.store(session)
    }

    /// The next free `<millis:016>-<n:02>` id.
    fn next_id(&self, at: Millis) -> SubmissionId {
        let mut n = 0u32;
        loop {
            let raw = format!("{at:016}-{n:02}");
            let id = SubmissionId::new(raw).expect("the allocated shape is a valid id");
            if !self.items.contains_key(&id) {
                return id;
            }
            n = n.saturating_add(1);
        }
    }
}

/// The id `submit` would allocate at `at`: `<millis:016>-<n:02>`, the
/// next free one in the ledger.
pub fn next_id(session: &Session, at: Millis) -> Result<SubmissionId, SubmissionError> {
    Ok(Ledger::parse(&session.driver_state)?.next_id(at))
}

/// Accepts a request as `Queued`. The caller saves before any side effect.
pub fn submit(
    session: &mut Session,
    kind: KindName,
    payload: serde_json::Value,
    at: Millis,
) -> Result<Record, SubmissionError> {
    let mut ledger = Ledger::parse(&session.driver_state)?;
    let id = ledger.next_id(at);
    let record = Record {
        id: id.clone(),
        kind,
        payload,
        status: Status::Queued,
        created_at: at,
        started_at: None,
        settled_at: None,
        owner: None,
        outcome: None,
        attempts: 0,
    };
    ledger.items.insert(id, StoredRecord::from(&record));
    ledger.finish(session)?;
    Ok(record)
}

/// `Queued` to `Running`: records the start, the owner, and one more
/// attempt. `payload` replaces the record's payload when given, which is
/// how a kind records state it only knows at start time.
pub fn start(
    session: &mut Session,
    id: &SubmissionId,
    at: Millis,
    payload: Option<serde_json::Value>,
) -> Result<Record, SubmissionError> {
    let mut ledger = Ledger::parse(&session.driver_state)?;
    let stored = ledger
        .items
        .get_mut(id)
        .ok_or_else(|| SubmissionError::NotFound(id.clone()))?;
    if stored.status != Status::Queued {
        return Err(SubmissionError::Transition {
            id: id.clone(),
            from: stored.status,
            to: Status::Running,
        });
    }
    stored.status = Status::Running;
    stored.started_at = Some(at);
    stored.owner = Some(Owner {
        pid: std::process::id(),
    });
    stored.attempts = stored.attempts.saturating_add(1);
    if let Some(payload) = payload {
        stored.payload = payload;
    }
    let record = stored.clone().into_record(id.clone());
    ledger.finish(session)?;
    Ok(record)
}

/// `Running` to the status the outcome names, `Done`, `Failed`, or
/// `Cancelled`, and records the outcome.
pub fn settle(
    session: &mut Session,
    id: &SubmissionId,
    outcome: Outcome,
    at: Millis,
) -> Result<Record, SubmissionError> {
    let to = match &outcome {
        Outcome::Done => Status::Done,
        Outcome::Failed { .. } => Status::Failed,
        Outcome::Cancelled => Status::Cancelled,
    };
    let mut ledger = Ledger::parse(&session.driver_state)?;
    let stored = ledger
        .items
        .get_mut(id)
        .ok_or_else(|| SubmissionError::NotFound(id.clone()))?;
    if stored.status != Status::Running {
        return Err(SubmissionError::Transition {
            id: id.clone(),
            from: stored.status,
            to,
        });
    }
    stored.status = to;
    stored.outcome = Some(outcome);
    stored.settled_at = Some(at);
    let record = stored.clone().into_record(id.clone());
    ledger.finish(session)?;
    Ok(record)
}

/// What [`cancel`] did with the record.
//
// The settled record is the answer the caller asked for and owns at
// once; boxing it would buy an allocation for a value handed over here
// and dropped by the caller.
#[allow(clippy::large_enum_variant)]
pub enum Cancel {
    /// The record was not in flight and is now `Cancelled`.
    Settled(Record),
    /// A live owner has it: the driver aborts its own task and settles.
    InFlight,
}

/// Cancels a record that has not started, or reports one that is in
/// flight.
///
/// Cancelling a `Running` record is not a write: the work lives in the
/// driver's task or a daemon, and only the driver can stop it. A cancel
/// that races a finishing turn loses to the settle that already wrote
/// its terminal status.
pub fn cancel(
    session: &mut Session,
    id: &SubmissionId,
    at: Millis,
) -> Result<Cancel, SubmissionError> {
    let mut ledger = Ledger::parse(&session.driver_state)?;
    let stored = ledger
        .items
        .get_mut(id)
        .ok_or_else(|| SubmissionError::NotFound(id.clone()))?;
    match stored.status {
        Status::Queued | Status::Interrupted => {
            stored.status = Status::Cancelled;
            stored.settled_at = Some(at);
            let record = stored.clone().into_record(id.clone());
            ledger.finish(session)?;
            Ok(Cancel::Settled(record))
        }
        Status::Running => Ok(Cancel::InFlight),
        other => Err(SubmissionError::Transition {
            id: id.clone(),
            from: other,
            to: Status::Cancelled,
        }),
    }
}

/// Starts an `Interrupted` record again, or a `Failed` one whose failure
/// was classed retryable.
///
/// `Interrupted` always retries: the process died and nothing else went
/// wrong. `Failed` retries only when its `outcome.retryable` is set; the
/// id's driver decided that at settle time, when it had the failure in
/// hand. A retried `Interrupted` record keeps its `started_at` and adds
/// one to `attempts`; a retried `Failed` record drops its `outcome`.
pub fn retry(
    session: &mut Session,
    id: &SubmissionId,
    at: Millis,
) -> Result<Record, SubmissionError> {
    let mut ledger = Ledger::parse(&session.driver_state)?;
    let stored = ledger
        .items
        .get_mut(id)
        .ok_or_else(|| SubmissionError::NotFound(id.clone()))?;
    match stored.status {
        Status::Interrupted => {
            stored.status = Status::Running;
            stored.settled_at = None;
            stored.attempts = stored.attempts.saturating_add(1);
        }
        Status::Failed => {
            let retryable = matches!(
                stored.outcome,
                Some(Outcome::Failed {
                    retryable: true,
                    ..
                })
            );
            if !retryable {
                return Err(SubmissionError::NotRetryable { id: id.clone() });
            }
            stored.status = Status::Running;
            stored.outcome = None;
            stored.settled_at = None;
        }
        other => {
            return Err(SubmissionError::Transition {
                id: id.clone(),
                from: other,
                to: Status::Running,
            });
        }
    }
    // The table writes neither timestamp on a retry: the record keeps
    // its original start time, and `at` stays in the signature so every
    // mutator takes one.
    let _ = at;
    let record = stored.clone().into_record(id.clone());
    ledger.finish(session)?;
    Ok(record)
}

/// What [`reconcile`] did to one stale record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The record's kind named a verdict, and the record now says it.
    Recovered {
        /// The stale record.
        id: SubmissionId,
        /// Its kind.
        kind: KindName,
        /// The status it now holds.
        to: Status,
    },
    /// The record's kind is not registered in this build; the record is
    /// `Interrupted`.
    Unclaimed {
        /// The stale record.
        id: SubmissionId,
        /// The tag no kind answers to.
        kind: KindName,
    },
}

/// Asks every registered kind what its stale records mean now.
///
/// A `Running` record loaded from disk is stale by construction: no
/// process of this one started it. The lifecycle asks the record's kind,
/// applies the verdict, and leaves terminal records alone. A record whose
/// kind this build does not link becomes `Interrupted` and is reported as
/// [`Change::Unclaimed`], never dropped.
///
/// Nothing is written when nothing changed, so a reconnect does not
/// rewrite a large session.
pub fn reconcile(
    session: &mut Session,
    kinds: &Kinds,
    at: Millis,
) -> Result<Vec<Change>, SubmissionError> {
    let mut ledger = Ledger::parse(&session.driver_state)?;
    // Adopt first: a tool that started work could not write the ledger,
    // so the work's own store is the only evidence it exists. Each kind
    // sees the ledger as it stands, including what earlier kinds adopted.
    for (_, kind) in kinds.iter() {
        let snapshot = Snapshot::from_records(ledger.records());
        let payloads = kind.adopt(&AdoptCtx {
            session,
            snapshot: &snapshot,
        });
        for payload in payloads {
            let id = ledger.next_id(at);
            let record = Record {
                id: id.clone(),
                kind: KindName::new(kind.name()).expect("registration validated the tag"),
                payload,
                status: Status::Running,
                created_at: at,
                started_at: Some(at),
                settled_at: None,
                owner: None,
                outcome: None,
                attempts: 1,
            };
            ledger.items.insert(id, StoredRecord::from(&record));
        }
    }
    let stale: Vec<SubmissionId> = ledger
        .items
        .iter()
        .filter(|(_, stored)| stored.status == Status::Running)
        .map(|(id, _)| id.clone())
        .collect();
    let mut changes = Vec::new();
    for id in stale {
        let Some(record) = ledger
            .items
            .get(&id)
            .map(|stored| stored.clone().into_record(id.clone()))
        else {
            continue;
        };
        let kind = record.kind.clone();
        let verdict = match kinds.get(&kind) {
            Some(registered) => registered.recover(&RecoverCtx {
                session,
                record: &record,
            }),
            None => {
                let stored = ledger.items.get_mut(&id).expect("the id was just read");
                stored.status = Status::Interrupted;
                stored.settled_at = Some(at);
                changes.push(Change::Unclaimed { id, kind });
                continue;
            }
        };
        let stored = ledger.items.get_mut(&id).expect("the id was just read");
        match verdict {
            Recovery::Done => {
                stored.status = Status::Done;
                stored.outcome = Some(Outcome::Done);
                stored.settled_at = Some(at);
            }
            Recovery::Queued => {
                stored.status = Status::Queued;
                stored.settled_at = None;
            }
            Recovery::Interrupted => {
                stored.status = Status::Interrupted;
                stored.settled_at = Some(at);
            }
            Recovery::Failed { error } => {
                stored.status = Status::Failed;
                stored.outcome = Some(Outcome::Failed {
                    error,
                    retryable: false,
                });
                stored.settled_at = Some(at);
            }
        }
        changes.push(Change::Recovered {
            id,
            kind,
            to: stored.status,
        });
    }
    if !changes.is_empty() {
        ledger.finish(session)?;
    }
    Ok(changes)
}
