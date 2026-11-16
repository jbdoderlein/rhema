//! The seam itself: four synchronous methods.

use rhema_core::Session;

use crate::{Meta, SessionId, StoreError};

/// A collection of saved sessions: one directory, one SQLite database.
///
/// Object-safe and thread-safe, so a frontend holds `Arc<dyn
/// SessionStore>` and a backend is chosen at compile time. Every method
/// is synchronous: the engine's [`rhema_core::CommitHook`] is called
/// inside a step, and a backend that blocks there is the existing deal.
/// Backends must not spawn their own work.
///
/// Absence is not failure: `load` of a never-saved id is `Ok(None)`,
/// `delete` of one is `Ok(false)`, and a store that has never been
/// written lists `Ok(vec![])`. A record whose body does not parse is
/// `Err` from `load` and still a row from `list`.
pub trait SessionStore: Send + Sync {
    /// The session saved under `id`; `None` when nothing was ever saved
    /// there. A record that exists but does not parse is `Err`, never
    /// `None`: the listing tolerates corruption, a resume surfaces it.
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;

    /// Atomically replace the session saved under `id`.
    ///
    /// On `Ok` the write is committed: a reader sees the old value or
    /// the new one, never a mix, and the backend's durability point has
    /// been reached (the file fsync and rename, or the committed
    /// transaction).
    fn save(&self, id: &SessionId, session: &Session) -> Result<(), StoreError>;

    /// Remove `id`. `Ok(true)` when a record existed and is gone,
    /// `Ok(false)` when there was none; removing a missing id is not an
    /// error.
    fn delete(&self, id: &SessionId) -> Result<bool, StoreError>;

    /// Every saved session in this store, newest first, ties broken by
    /// id ascending. One corrupt record never fails the listing: it
    /// still lists, with `opening_prompt: None`.
    fn list(&self) -> Result<Vec<Meta>, StoreError>;
}
