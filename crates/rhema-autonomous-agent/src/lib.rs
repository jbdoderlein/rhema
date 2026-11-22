//! `rhema-autonomous-agent`, the resident-agent core tier: the
//! source-driven session loop, the
//! `Source` contract for deterministic detection, and the lease-driven
//! handler loop, all over `rhema-rsd`'s dispatcher.
//!
//! The crate knows sources, sessions, and a driver, no specific
//! integration and no tool machinery: tool composition is the
//! consumer's, done over `rhema_core`'s `ToolResolver` seam and handed
//! to its handler. A [**Source**](source::Source) detects (never
//! reasons): its `detect` pass is deterministic code that polls the
//! outside world and ingests events; the model is only ever reached
//! inside a [**Handler**](driver::Handler) run, after the dispatcher
//! granted a lease. A [**Driver**](driver::Driver) turns admitted keys
//! into handler runs, acquire, live inbox, checkpoint, release, the
//! spec §3 loop with `poll_inbox` batched at step boundaries and a
//! checkpoint per round.
//!
//! Deliberately deferred, with the reasoning recorded where the decision
//! bites: classification profiles (`ClassifyFn`) stay at the source
//! boundary, `rhema-rsd` ships without them (its documented §9
//! deviation), and no second consumer needs the shared vocabulary yet.
//!
//! Layout:
//! - [`source`]: the `Source` contract: deterministic detection into the dispatcher
//! - [`driver`]: the `Driver`, `Handler`, and `SessionContext`: the lease-driven loop

#![forbid(unsafe_code)]

pub mod driver;
pub mod source;

// Re-exported so implementors of the async seams can apply #[async_trait]
// without adding the crate themselves.
pub use async_trait::async_trait;
pub use driver::{Driver, Handler, SessionContext};
pub use source::{Source, SourceError};
