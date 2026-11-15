//! The Brave Search API adapter: one vendor's answer to the
//! [`WebSearchProvider`](rhema_websearch::WebSearchProvider) seam.
//!
//! Brave's API is a GET with the query and the count as URL
//! parameters, the key in a `X-Subscription-Token` header, and the
//! hits under a `web` key on the web route (the news route puts them
//! at the top level). Two shapes this adapter pins (vendor docs are
//! the source of truth, `specs/rhema-websearch.md` §4):
//!
//! - `text_decorations=false`: without it Brave decorates
//!   descriptions with `<strong>` tags and HTML entities, and the
//!   seam's snippets are plaintext by contract.
//! - a 200 response whose body carries no `web` key (no `results`
//!   array on the news route) reads as zero results, not a parse
//!   failure: a vendor that answered with nothing is an honest
//!   answer, and a `Parse` error would bill the next provider in the
//!   chain for a query the first one settled.
//!
//! Layout: the [`wire`] types and the pure mapping (always compiled),
//! and the [`BraveProvider`] over reqwest (feature `native`).

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{BRAVE_NEWS_PATH, BRAVE_WEB_PATH, BraveProvider, DEFAULT_BASE_URL, USER_AGENT};

pub use wire::{BraveResponse, BraveResult, results_from_response};
