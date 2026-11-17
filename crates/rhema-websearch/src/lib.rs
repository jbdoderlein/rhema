//! `rhema-websearch`, the vocabulary and the fallback chain of rhema
//! web search.
//!
//! Every `rhema-websearch-*` crate implements the [`WebSearchProvider`]
//! seam against one vendor's API; this crate holds only what they all
//! share: the [`SearchQuery`] a caller builds, the
//! [`SearchResult`](crate::result::SearchResult) list an adapter
//! answers, the [`SearchError`](crate::error::SearchError) classes an
//! adapter reports, and the [`Chain`] that tries a frontend's
//! configured providers in order and stops at the first success. No
//! HTTP here: the transport stays in the adapters, reqwest behind a
//! `native` feature each, so a future host-fetch adapter for a worker
//! runtime implements the same trait behind its own feature.
//!
//! The chain is the product's policy, not the caller's: sequential
//! attempts, every error falls through, first success wins, an empty
//! result set is a success (a provider that answered honestly is not
//! a reason to bill a second one), budgets under the shell tool's own
//! command timeout so the model reads the chain's error text instead
//! of a killed command's. Spec: `specs/rhema-websearch.md` §5.
//!
//! The macro that reaches the model (the `search` bash function) is
//! the frontend's wiring (`rhema-bash::websearch` plus the
//! `__search` mode of the reference frontend); the claw and the cloud
//! harness assemble their own agents and hold a `Chain` behind a
//! trait object, which is the whole point of the seam.
//!
//! Layout:
//! - [`query`]: the search request
//! - [`result`]: one hit: title, url, snippet
//! - [`error`]: the failure classes and the chain's aggregate
//! - [`provider`]: the adapter seam
//! - [`chain`]: ordered fallback, first success wins
//! - [`render`]: the markdown a `search` command prints
//! - [`mock`]: the scripted provider (feature `mock`)

#![forbid(unsafe_code)]

pub mod chain;
pub mod error;
pub mod provider;
pub mod query;
pub mod render;
pub mod result;

#[cfg(feature = "mock")]
pub mod mock;

pub use chain::{Chain, ChainOutcome};
pub use error::{ChainError, SearchError};
pub use provider::WebSearchProvider;
pub use query::SearchQuery;
pub use render::render;
pub use result::SearchResult;

#[cfg(feature = "mock")]
pub use mock::MockSearchProvider;
