//! The generic lifecycle: the ledger's shape, the state machine, retention,
//! and the retry gate.

use rhema_core::Session;
use rhema_session_store::SessionStore;
use rhema_session_store_file::FileStore;
use rhema_submissions::{
    Cancel, Change, KindName, Kinds, Outcome, Status, SubmissionError, cancel, reconcile, retry,
    settle, snapshot, start, submit,
};
use serde_json::json;

fn session_with_state(state: serde_json::Value) -> Session {
    Session {
        driver_state: state,
        ..Session::default()
    }
}

fn kind(name: &str) -> KindName {
    KindName::new(name).unwrap()
}

fn state(session: &Session) -> serde_json::Value {
    session.driver_state.clone()
}

#[test]
fn the_ledger_round_trips_through_the_file_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path());
    let mut session = session_with_state(json!({ "rsd": { "key": "interactive" } }));

    let record = submit(
        &mut session,
        kind("turn"),
        json!({ "input": "fix the test" }),
        10,
    )
    .unwrap();
    let started = start(&mut session, &record.id, 20, None).unwrap();
    let settled = settle(&mut session, &started.id, Outcome::Done, 30).unwrap();
    // A kind and a payload this build does not link must survive too.
    let unknown = submit(
        &mut session,
        kind("job"),
        json!({ "command": "cargo test" }),
        40,
    )
    .unwrap();

    store.save(&record.id, &session).unwrap();
    let loaded = store
        .load(&record.id)
        .unwrap()
        .expect("the session was saved");

    assert_eq!(
        loaded.driver_state["rsd"]["key"], "interactive",
        "another driver's driver_state key survives"
    );
    let snapshot = snapshot(&loaded).unwrap();
    assert_eq!(snapshot.get(&settled.id), Some(&settled));
    assert_eq!(
        snapshot.get(&unknown.id).map(|record| &record.payload),
        Some(&json!({ "command": "cargo test" }))
    );
    assert_eq!(
        snapshot.newest().map(|record| &record.id),
        Some(&unknown.id)
    );

    // The stored item is the §2 shape: the id is the key, not a field.
    let item = &session.driver_state["submissions"]["items"][settled.id.as_str()];
    assert_eq!(item["status"], "done");
    assert_eq!(item["kind"], "turn");
    assert!(item.get("id").is_none(), "the id is the key: {item}");
    assert_eq!(session.driver_state["submissions"]["v"], 1);
}

#[test]
fn a_session_from_before_this_crate_is_left_alone() {
    // `driver_state: null`, no ledger: reads as empty, and nothing is
    // stamped until the first submit.
    let mut session = Session::default();
    assert!(snapshot(&session).unwrap().is_empty());
    assert!(
        rhema_submissions::reconcile(&mut session, &rhema_submissions::Kinds::new(), 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.driver_state, serde_json::Value::Null);

    submit(&mut session, kind("turn"), json!({ "input": "go" }), 2).unwrap();
    assert_eq!(session.driver_state["submissions"]["v"], 1);
}

#[test]
fn the_state_machine_rejects_and_leaves_the_record_alone() {
    let mut session = Session::default();
    let queued = submit(&mut session, kind("turn"), json!({}), 1).unwrap();
    let before = state(&session);
    assert_eq!(
        settle(&mut session, &queued.id, Outcome::Done, 2),
        Err(SubmissionError::Transition {
            id: queued.id.clone(),
            from: Status::Queued,
            to: Status::Done,
        })
    );
    assert_eq!(state(&session), before, "a rejected settle writes nothing");

    let running = start(&mut session, &queued.id, 3, None).unwrap();
    let done = settle(&mut session, &running.id, Outcome::Done, 4).unwrap();
    let before = state(&session);
    assert_eq!(
        start(&mut session, &done.id, 5, None),
        Err(SubmissionError::Transition {
            id: done.id.clone(),
            from: Status::Done,
            to: Status::Running,
        })
    );
    assert!(matches!(
        cancel(&mut session, &done.id, 6),
        Err(SubmissionError::Transition {
            from: Status::Done,
            to: Status::Cancelled,
            ..
        })
    ));
    assert_eq!(state(&session), before);
}

#[test]
fn a_queued_cancel_settles_and_a_running_cancel_does_not() {
    let mut session = Session::default();
    let queued = submit(&mut session, kind("turn"), json!({}), 1).unwrap();
    let Cancel::Settled(cancelled) = cancel(&mut session, &queued.id, 2).unwrap() else {
        panic!("a queued record settles on cancel");
    };
    assert_eq!(cancelled.status, Status::Cancelled);
    assert_eq!(cancelled.settled_at, Some(2));

    let next = submit(&mut session, kind("turn"), json!({}), 3).unwrap();
    let running = start(&mut session, &next.id, 4, None).unwrap();
    let before = state(&session);
    assert!(matches!(
        cancel(&mut session, &running.id, 5).unwrap(),
        Cancel::InFlight
    ));
    assert_eq!(state(&session), before, "a running cancel is not a write");
}

#[test]
fn a_newer_ledger_and_a_corrupt_one_are_typed_errors() {
    let newer = session_with_state(json!({
        "submissions": { "v": 2, "items": {} }
    }));
    assert!(matches!(
        snapshot(&newer),
        Err(SubmissionError::Unsupported(2))
    ));

    let corrupt = session_with_state(json!({
        "submissions": {
            "v": 1,
            "items": {
                "0001759000000000-00": {
                    "kind": "turn",
                    "payload": { "input": "hi" },
                    "status": "sleeping",
                    "created_at": 1
                }
            }
        }
    }));
    let before = state(&corrupt);
    assert!(matches!(
        snapshot(&corrupt),
        Err(SubmissionError::Corrupt(_))
    ));
    assert_eq!(state(&corrupt), before, "a rejected read writes nothing");

    // The same for a mutator that would have written.
    let mut corrupt = corrupt;
    assert!(matches!(
        submit(&mut corrupt, kind("turn"), json!({}), 2),
        Err(SubmissionError::Corrupt(_))
    ));
    assert_eq!(state(&corrupt), before);
}

#[test]
fn retention_keeps_every_unsettled_record_and_the_newest_32_settled() {
    let mut session = Session::default();
    // One unsettled record with the oldest id in the ledger.
    let early = submit(&mut session, kind("turn"), json!({}), 0).unwrap();
    for at in 1..=40 {
        let record = submit(&mut session, kind("turn"), json!({ "at": at }), at).unwrap();
        let record = start(&mut session, &record.id, at, None).unwrap();
        settle(&mut session, &record.id, Outcome::Done, at).unwrap();
    }

    let snapshot = snapshot(&session).unwrap();
    assert_eq!(snapshot.len(), 33, "32 settled plus the one unsettled");
    assert_eq!(
        snapshot.get(&early.id).map(|record| record.status),
        Some(Status::Queued),
        "an unsettled record is never dropped"
    );
    let settled: Vec<_> = snapshot
        .iter()
        .filter(|record| record.status.is_terminal())
        .collect();
    assert_eq!(settled.len(), 32);
    assert_eq!(
        settled.first().map(|record| record.id.as_str()),
        Some("0000000000000009-00"),
        "the oldest settled records went first"
    );
}

#[test]
fn retry_is_gated_on_the_settle_time_verdict() {
    let mut session = Session::default();

    let permanent = submit(&mut session, kind("job"), json!({}), 1).unwrap();
    let permanent = start(&mut session, &permanent.id, 2, None).unwrap();
    let permanent = settle(
        &mut session,
        &permanent.id,
        Outcome::Failed {
            error: "the test failed".into(),
            retryable: false,
        },
        3,
    )
    .unwrap();
    let before = state(&session);
    assert_eq!(
        retry(&mut session, &permanent.id, 4),
        Err(SubmissionError::NotRetryable {
            id: permanent.id.clone()
        })
    );
    assert_eq!(state(&session), before);

    let retryable = submit(&mut session, kind("job"), json!({}), 5).unwrap();
    let retryable = start(&mut session, &retryable.id, 6, None).unwrap();
    let retryable = settle(
        &mut session,
        &retryable.id,
        Outcome::Failed {
            error: "the daemon died".into(),
            retryable: true,
        },
        7,
    )
    .unwrap();
    let again = retry(&mut session, &retryable.id, 8).unwrap();
    assert_eq!(again.status, Status::Running);
    assert_eq!(again.outcome, None, "the retried failure's verdict is gone");
    assert_eq!(again.settled_at, None);
    assert_eq!(again.attempts, 1, "a retried failure keeps its attempts");

    // An interrupted record always retries, and counts the attempt.
    let queued = submit(&mut session, kind("job"), json!({}), 9).unwrap();
    let running = start(&mut session, &queued.id, 10, None).unwrap();
    assert_eq!(running.attempts, 1);
    // A process that died holding it, and a build that links no `job`
    // kind: the record reconciles to Interrupted.
    let changes = reconcile(&mut session, &Kinds::new(), 11).unwrap();
    assert!(matches!(changes[0], Change::Unclaimed { .. }));
    let interrupted = snapshot(&session)
        .unwrap()
        .get(&running.id)
        .cloned()
        .unwrap();
    assert_eq!(interrupted.status, Status::Interrupted);
    let retried = retry(&mut session, &running.id, 12).unwrap();
    assert_eq!(retried.status, Status::Running);
    assert_eq!(retried.attempts, 2);
    assert_eq!(retried.settled_at, None);
}
