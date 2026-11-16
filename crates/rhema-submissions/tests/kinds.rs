//! The kind seam: an unknown kind reconciles safely, a registered one
//! decides, and reconcile is idempotent.

use std::sync::{Arc, Mutex};

use rhema_core::Session;
use rhema_submissions::{
    Change, Kind, KindName, Kinds, Record, RecoverCtx, Recovery, Status, reconcile, snapshot,
    start, submit,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A kind with a fixed verdict that remembers the payloads it read.
struct TestKind {
    verdict: Recovery,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Seen {
    n: u64,
}

impl Kind for TestKind {
    fn name(&self) -> &'static str {
        "test"
    }

    fn recover(&self, ctx: &RecoverCtx<'_>) -> Recovery {
        let payload: Seen = ctx.payload().expect("the payload is this kind's schema");
        self.seen.lock().unwrap().push(payload);
        self.verdict
    }
}

fn registered(verdict: Recovery) -> (Kinds, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kinds = Kinds::new().with(Arc::new(TestKind {
        verdict,
        seen: seen.clone(),
    }));
    (kinds, seen)
}

fn running(tag: &str, payload: serde_json::Value) -> (Session, Record) {
    let mut session = Session::default();
    let queued = submit(&mut session, KindName::new(tag).unwrap(), payload, 1).unwrap();
    let record = start(&mut session, &queued.id, 2, None).unwrap();
    (session, record)
}

#[test]
fn an_unregistered_kind_interrupts_and_is_reported() {
    let (mut session, record) = running("job", json!({ "command": "cargo test" }));
    let before = snapshot(&session)
        .unwrap()
        .get(&record.id)
        .cloned()
        .unwrap();

    let changes = reconcile(&mut session, &Kinds::new(), 3).unwrap();

    let kind = KindName::new("job").unwrap();
    assert_eq!(
        changes,
        [Change::Unclaimed {
            id: record.id.clone(),
            kind: kind.clone(),
        }]
    );
    let after = snapshot(&session).unwrap();
    let record = after.get(&record.id).unwrap();
    assert_eq!(record.status, Status::Interrupted);
    assert_eq!(record.settled_at, Some(3));
    assert_eq!(
        record.payload, before.payload,
        "the payload is never touched"
    );
    assert_eq!(record.kind, kind);
}

#[test]
fn each_verdict_lands_and_the_kind_sees_its_own_payload() {
    for (verdict, expected) in [
        (Recovery::Done, Status::Done),
        (Recovery::Queued, Status::Queued),
        (Recovery::Interrupted, Status::Interrupted),
    ] {
        let (kinds, seen) = registered(verdict);
        let (mut session, record) = running("test", json!({ "n": 7 }));

        let changes = reconcile(&mut session, &kinds, 3).unwrap();

        assert_eq!(
            changes,
            [Change::Recovered {
                id: record.id.clone(),
                kind: KindName::new("test").unwrap(),
                to: expected,
            }]
        );
        assert_eq!(*seen.lock().unwrap(), [Seen { n: 7 }]);
        let after = snapshot(&session).unwrap();
        let record = after.get(&record.id).unwrap();
        assert_eq!(record.status, expected);
        match expected {
            Status::Queued => assert_eq!(record.settled_at, None),
            _ => assert_eq!(record.settled_at, Some(3)),
        }
    }
}

#[test]
fn reconcile_writes_nothing_on_the_second_pass() {
    let (kinds, _) = registered(Recovery::Interrupted);
    let (mut session, _) = running("test", json!({ "n": 1 }));
    reconcile(&mut session, &kinds, 3).unwrap();
    let after_first = session.driver_state.clone();

    assert!(reconcile(&mut session, &kinds, 4).unwrap().is_empty());
    assert_eq!(session.driver_state, after_first);

    // The same for an unclaimed record.
    let (mut session, _) = running("job", json!({}));
    reconcile(&mut session, &kinds, 3).unwrap();
    let after_first = session.driver_state.clone();
    assert!(reconcile(&mut session, &kinds, 4).unwrap().is_empty());
    assert_eq!(session.driver_state, after_first);
}

#[test]
fn reconcile_leaves_terminal_records_alone() {
    // Nothing to ask: no record is running.
    let mut session = Session::default();
    let queued = submit(&mut session, KindName::new("test").unwrap(), json!({}), 1).unwrap();
    submit(&mut session, KindName::new("job").unwrap(), json!({}), 2).unwrap();
    let before = session.driver_state.clone();
    assert!(
        reconcile(&mut session, &Kinds::new(), 3)
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.driver_state, before);
    assert_eq!(
        snapshot(&session).unwrap().get(&queued.id).unwrap().status,
        Status::Queued
    );
}

#[test]
#[should_panic(expected = "is not a valid tag")]
fn a_kinds_map_rejects_a_tag_outside_the_grammar() {
    struct BadKind;
    impl Kind for BadKind {
        fn name(&self) -> &'static str {
            "Not_A_Tag"
        }
        fn recover(&self, _ctx: &RecoverCtx<'_>) -> Recovery {
            Recovery::Interrupted
        }
    }
    Kinds::new().with(Arc::new(BadKind));
}
