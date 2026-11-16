# rhema-session-store

The seam saved-session backends implement: one small synchronous trait,
the validated session id, the picker row, the error type, and a
replayable contract suite.

```rust
pub trait SessionStore: Send + Sync {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    fn save(&self, id: &SessionId, session: &Session) -> Result<(), StoreError>;
    fn delete(&self, id: &SessionId) -> Result<bool, StoreError>;
    fn list(&self) -> Result<Vec<Meta>, StoreError>;
}
```

`save` returning `Ok` means the backend reached its durability point. A
missing id reads as absence (`Ok(None)`, `Ok(false)`, `Ok(vec![])`), and
a corrupt body fails `load` while still listing in `list`.

The methods are synchronous because the engine calls its commit hook
inside a step. A backend blocks the turn task; no backend spawns.

This crate carries no backend. `rhema-session-store-file` is the
reference implementation (one bare-JSON `Session` per file) and
`rhema-session-store-sqlite` is the row-based one.

## The contract suite

Enable the `store-contract` feature from a backend's dev-dependencies
and replay the suite against the store:

```toml
[dev-dependencies]
rhema-session-store = { path = "../rhema-session-store", version = "0.3.0", features = ["store-contract"] }
```

```rust
#[test]
fn the_contract_suite_passes() {
    rhema_session_store::contract::run_all(&my_store);
}
```

`run_all` expects a fresh store. Each assertion function takes any
`&dyn SessionStore` and uses its own id namespace, so a backend can also
call them one at a time.

Spec: `specs/rhema-session-store.md`.
