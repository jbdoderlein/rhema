//! The Tavily search API adapter: one vendor's answer to the
//! [`WebSearchProvider`](rhema_websearch::WebSearchProvider) seam.
//!
//! Tavily's API is a POST with a JSON body, the key in a bearer
//! `Authorization` header, and the results in a `results` array whose
//! hits carry the snippet directly (`content`, a relevance-picked
//! chunk, no separate contents call). Four shapes this adapter pins
//! (vendor docs are the source of truth, `specs/rhema-websearch.md`
//! §4):
//!
//! - `include_answer: false` and `include_raw_content: false`: the
//!   seam wants ranked snippets, not a vendor LLM's paragraph or
//!   whole pages, and both extras cost the same call more.
//! - `search_depth: "basic"`: the basic depth is one credit, the
//!   advanced is two; the chain's retries should not double a bill.
//! - `topic` carries the news flag: `"news"` for the news index,
//!   `"general"` otherwise.
//! - a 200 response whose `results` array is absent reads as zero
//!   results, not a parse failure: a vendor that answered with
//!   nothing is an honest answer, and a `Parse` error would bill the
//!   next provider in the chain for a query the first one settled.
//!
//! Layout: the [`wire`] types and the pure mapping (always compiled),
//! and the [`TavilyProvider`] over reqwest (feature `native`).

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{DEFAULT_BASE_URL, SEARCH_PATH, TavilyProvider, USER_AGENT};

pub use wire::{TavilyResponse, TavilyResult, results_from_response};
