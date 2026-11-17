//! The replayable [`SessionStore`] behavioral suite.
//!
//! Every function here is a plain assertion-based test body taking any
//! `&dyn SessionStore`. A backend dev-depends on this crate with
//! `features = ["store-contract"]` and calls them from its own `#[test]`s;
//! the contract is the trait's docs made executable.
//!
//! The functions use their own id namespace, so one store can run them in
//! any order. [`run_all`] expects a fresh store: the ordering test reads
//! every row in the listing.

use std::time::Duration;

use rhema_core::{Content, Message, Role, Session};

use crate::{SessionId, SessionStore};

/// An id the suite owns, valid by construction.
fn id(raw: &str) -> SessionId {
    SessionId::new(raw).expect("the suite's ids are valid")
}

/// A session with one user prompt, a tool call and its result, and both
/// opaque blobs filled, so a round trip proves more than the history.
fn session(prompt: &str) -> Session {
    Session {
        history: vec![
            Message::text(Role::User, prompt),
            Message {
                annotations: Default::default(),
                role: Role::Assistant,
                content: vec![Content::ToolCall(rhema_core::ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    args: serde_json::json!({ "command": "echo hi" }),
                    provider_data: serde_json::Value::Null,
                })],
            },
            Message {
                annotations: Default::default(),
                role: Role::Tool,
                content: vec![Content::ToolResult(rhema_core::ToolResult::success(
                    "call-1", "hi",
                ))],
            },
        ],
        driver_state: serde_json::json!({ "key": "contract" }),
        cost_usd: 1.25,
        usage_stats: serde_json::json!({ "requests": 2 }),
    }
}

/// [`SessionStore::load`] on a never-written id returns `None`.
pub fn load_missing_returns_none(store: &dyn SessionStore) {
    let missing = id("contract-missing");
    assert_eq!(
        store.load(&missing).unwrap(),
        None,
        "an absent record is absence, not an error"
    );
}

/// A saved session survives `save` + `load` field for field.
pub fn save_then_load_round_trips(store: &dyn SessionStore) {
    let contract = id("contract-round-trip");
    let saved = session("round trip");
    store.save(&contract, &saved).unwrap();
    assert_eq!(store.load(&contract).unwrap(), Some(saved));
}

/// A second `save` replaces the whole value; no field of the first
/// survives.
pub fn save_replaces_whole_value(store: &dyn SessionStore) {
    let contract = id("contract-replace");
    store.save(&contract, &session("first value")).unwrap();
    let replacement = Session {
        history: vec![Message::text(Role::User, "second value")],
        driver_state: serde_json::Value::Null,
        cost_usd: 0.0,
        usage_stats: serde_json::Value::Null,
    };
    store.save(&contract, &replacement).unwrap();
    assert_eq!(store.load(&contract).unwrap(), Some(replacement));
}

/// `delete` reports whether a record was there and removes it either
/// way.
pub fn delete_removes_and_reports(store: &dyn SessionStore) {
    let contract = id("contract-delete");
    store.save(&contract, &session("delete me")).unwrap();
    assert!(store.delete(&contract).unwrap(), "the record existed");
    assert_eq!(store.load(&contract).unwrap(), None);
    assert!(
        !store.delete(&contract).unwrap(),
        "removing a missing id is Ok(false), not an error"
    );
}

/// `list` is newest first, ties broken by id ascending, and a row
/// carries the body's length and opening prompt.
pub fn list_is_newest_first_and_ordered(store: &dyn SessionStore) {
    let older = id("contract-list-older");
    let newer = id("contract-list-newer");
    store.save(&older, &session("older prompt")).unwrap();
    // The contract's ordering is by millisecond save time; two saves in
    // the same millisecond would tie and the id tie-break would take
    // the assertion away from what it tests.
    std::thread::sleep(Duration::from_millis(2));
    store.save(&newer, &session("newer prompt")).unwrap();

    let rows = store.list().unwrap();
    let position = |wanted: &SessionId| {
        rows.iter()
            .position(|meta| &meta.id == wanted)
            .unwrap_or_else(|| panic!("{wanted} is missing from the listing"))
    };
    assert!(
        position(&newer) < position(&older),
        "the newer save lists first: {rows:?}"
    );
    for pair in rows.windows(2) {
        assert!(
            pair[0].saved_at > pair[1].saved_at
                || (pair[0].saved_at == pair[1].saved_at && pair[0].id < pair[1].id),
            "listing order is saved_at descending then id ascending: {pair:?}"
        );
    }
    let row = &rows[position(&newer)];
    assert_eq!(row.opening_prompt.as_deref(), Some("newer prompt"));
    assert!(row.bytes > 0, "a saved body has a length");
}

/// Runs the whole suite; pass a fresh store.
pub fn run_all(store: &dyn SessionStore) {
    load_missing_returns_none(store);
    save_then_load_round_trips(store);
    save_replaces_whole_value(store);
    delete_removes_and_reports(store);
    list_is_newest_first_and_ordered(store);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use std::collections::BTreeMap;

    use super::*;
    use crate::{Meta, StoreError, opening_prompt};

    /// An in-memory store in the suite's own file, so the suite is
    /// exercised from the seam crate as well as from each backend.
    #[derive(Default)]
    struct MemoryStore {
        rows: Mutex<BTreeMap<String, (u64, Session)>>,
        clock: Mutex<u64>,
    }

    impl SessionStore for MemoryStore {
        fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(id.as_str())
                .map(|(_, session)| session.clone()))
        }

        fn save(&self, id: &SessionId, session: &Session) -> Result<(), StoreError> {
            let mut clock = self.clock.lock().unwrap();
            *clock += 1;
            self.rows
                .lock()
                .unwrap()
                .insert(id.as_str().to_owned(), (*clock, session.clone()));
            Ok(())
        }

        fn delete(&self, id: &SessionId) -> Result<bool, StoreError> {
            Ok(self.rows.lock().unwrap().remove(id.as_str()).is_some())
        }

        fn list(&self) -> Result<Vec<Meta>, StoreError> {
            let rows = self.rows.lock().unwrap();
            let mut out: Vec<Meta> = rows
                .iter()
                .map(|(id, (saved_at, session))| Meta {
                    id: SessionId::new(id.clone()).unwrap(),
                    saved_at: *saved_at,
                    bytes: 1,
                    opening_prompt: opening_prompt(session),
                })
                .collect();
            out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
            Ok(out)
        }
    }

    #[test]
    fn the_suite_passes_on_a_memory_store() {
        run_all(&MemoryStore::default());
    }
}
