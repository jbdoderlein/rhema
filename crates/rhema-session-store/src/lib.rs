//! `rhema-session-store`, the seam every saved-session backend implements.
//!
//! A frontend keeps one conversation as a [`rhema_core::Session`]. This
//! crate holds the four things a backend and its consumers agree on: the
//! [`SessionStore`] trait, the validated [`SessionId`], the [`Meta`] row a
//! picker lists, and the [`StoreError`] every failure arrives as. The
//! backends live in their own crates (`rhema-session-store-file`,
//! `rhema-session-store-sqlite`), the same one-backend-per-crate shape as
//! `rhema-provider-*` and `rhema-websearch-*`, so a frontend pulls only the
//! store it uses.
//!
//! The trait is synchronous because the engine's
//! [`rhema_core::CommitHook`] is: the backend blocks on the turn task, and
//! no backend spawns. `save` returning `Ok` means the backend's durability
//! point was reached: a reader sees the old value or the new one, never a
//! mix, and a crash after the return loses nothing. A missing id is
//! absence, not an error; corruption surfaces on `load` and is tolerated
//! by `list`.
//!
//! Layout:
//! - [`id`]: the validated, file-safe [`SessionId`]
//! - [`error`]: [`StoreError`] and its `CommitError` conversion
//! - [`meta`]: the listing row
//! - [`prompt`]: the opening-prompt helper and the shared truncation
//! - [`store`]: the [`SessionStore`] trait
//! - `contract`: the replayable behavioral suite (feature
//!   `store-contract`)

#![forbid(unsafe_code)]

pub mod error;
pub mod id;
pub mod meta;
pub mod prompt;
pub mod store;

#[cfg(feature = "store-contract")]
pub mod contract;

pub use error::StoreError;
pub use id::SessionId;
pub use meta::Meta;
pub use prompt::{OPENING_PROMPT_WIDTH, opening_prompt, truncate_chars};
pub use store::SessionStore;
