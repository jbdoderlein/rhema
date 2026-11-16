//! The record: one requested unit of work, its kind tag, and its status.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::error::SubmissionError;
use rhema_session_store::SessionId;

/// The id a caller holds across a disconnect.
///
/// It is the session store's validated [`SessionId`]: one alphabet in
/// the framework, and an id already safe to name a child session with.
pub type SubmissionId = SessionId;

/// Milliseconds since the epoch. Every mutator takes one, so tests pass
/// fixed values and never sleep.
pub type Millis = u64;

/// The longest kind tag, in bytes.
pub const MAX_KIND_NAME: usize = 32;

/// A kind tag: `[a-z0-9][a-z0-9-]{0,31}`, validated at construction.
///
/// It is not an enum. Any crate may mint one, and a build that does not
/// link the kind still reads its records, as unclaimed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KindName(String);

impl KindName {
    /// Validates `name` and wraps it.
    pub fn new(name: &str) -> Result<Self, SubmissionError> {
        let mut bytes = name.bytes();
        let head_ok = matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'));
        let tail_ok = bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'));
        if !head_ok || !tail_ok || name.len() > MAX_KIND_NAME {
            return Err(SubmissionError::InvalidKind(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    /// The tag as written into a record.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KindName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for KindName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

struct KindVisitor;

impl Visitor<'_> for KindVisitor {
    type Value = KindName;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a lowercase kind tag")
    }

    fn visit_str<E: de::Error>(self, raw: &str) -> Result<Self::Value, E> {
        KindName::new(raw).map_err(|err| E::custom(err.to_string()))
    }
}

impl<'de> Deserialize<'de> for KindName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(KindVisitor)
    }
}

/// Where a record is in its life.
///
/// A status names who owns the record, not what the work is doing: an ask
/// blocked on the user is still `Running`, and its payload says what it
/// waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted and durable, no work started.
    Queued,
    /// A live owner has it: working, or blocked on something it waits
    /// for.
    Running,
    /// The kind says the work finished.
    Done,
    /// The kind says the work ended in an error.
    Failed,
    /// The driver cancelled it before it finished.
    Cancelled,
    /// A `Running` record found by a process that did not start it, and
    /// whose kind could not say it finished.
    Interrupted,
}

impl Status {
    /// Whether the record's life is over: `Done`, `Failed`, or
    /// `Cancelled`.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        };
        f.write_str(name)
    }
}

/// How a record ended.
///
/// The result itself is not copied: a job's result is in the daemon, a
/// turn's in history, a subagent's in the child session. `Done` carries
/// no answer for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Finished; the kind derives the result from its own store.
    Done,
    /// The work ended in an error. `retryable` is the driver's verdict at
    /// settle time, when it still has the failure in hand: a dead network
    /// is retryable, a spent budget or a failed test is not.
    Failed {
        /// The failure as the driver reported it.
        error: String,
        /// Whether `retry` may start the record again.
        retryable: bool,
    },
    /// The driver stopped the work.
    Cancelled,
}

/// Which process claimed a record. Diagnostics only: no decision reads
/// it, because the store cannot keep a lock a stale process honors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// The owner's process id.
    pub pid: u32,
}

/// One request and its life.
///
/// The ledger stores these fields under the record's id, without a
/// nested `id`; this shape is for a caller that hands one record around.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The id the caller holds; also the key in the ledger.
    pub id: SubmissionId,
    /// The tag of the crate that owns the work.
    pub kind: KindName,
    /// The kind's own request, opaque to this crate.
    pub payload: serde_json::Value,
    /// Where the record is in its life.
    pub status: Status,
    /// When it was accepted.
    pub created_at: Millis,
    /// When it was first started; `None` while queued.
    pub started_at: Option<Millis>,
    /// When it reached its current terminal status.
    pub settled_at: Option<Millis>,
    /// Which process claimed it, for diagnosis.
    pub owner: Option<Owner>,
    /// How it ended, when it did.
    pub outcome: Option<Outcome>,
    /// How many times it was started; 1 for a normal life.
    pub attempts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_tags_follow_the_grammar() {
        for name in ["turn", "job", "ask-2", "a1-b2-c3"] {
            assert_eq!(KindName::new(name).unwrap().as_str(), name);
        }
        for name in ["", "Turn", "-job", "job_2", "job/2", "a b", "é"] {
            assert!(
                matches!(KindName::new(name), Err(SubmissionError::InvalidKind(_))),
                "{name:?} must be rejected"
            );
        }
        assert!(KindName::new(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn the_kind_tag_round_trips_and_is_validated_on_the_way_in() {
        let kind = KindName::new("deploy").unwrap();
        assert_eq!(serde_json::to_string(&kind).unwrap(), "\"deploy\"");
        assert_eq!(
            serde_json::from_str::<KindName>("\"deploy\"").unwrap(),
            kind
        );
        assert!(serde_json::from_str::<KindName>("\"Deploy\"").is_err());
    }

    #[test]
    fn the_status_names_are_snake_case() {
        for (status, name) in [
            (Status::Queued, "queued"),
            (Status::Running, "running"),
            (Status::Done, "done"),
            (Status::Failed, "failed"),
            (Status::Cancelled, "cancelled"),
            (Status::Interrupted, "interrupted"),
        ] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{name}\"")
            );
            assert_eq!(status.to_string(), name);
        }
    }

    #[test]
    fn the_outcome_shapes_round_trip() {
        let done = Outcome::Done;
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({ "kind": "done" })
        );
        let failed = Outcome::Failed {
            error: "dead link".into(),
            retryable: true,
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            serde_json::json!({ "kind": "failed", "error": "dead link", "retryable": true })
        );
        assert_eq!(
            serde_json::from_value::<Outcome>(serde_json::to_value(&failed).unwrap()).unwrap(),
            failed
        );
    }
}
