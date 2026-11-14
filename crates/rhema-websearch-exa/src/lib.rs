//! The Exa search API adapter: one vendor's answer to the
//! [`WebSearchProvider`](rhema_websearch::WebSearchProvider) seam.
//!
//! Exa's API is a POST with a JSON body, the key in an `x-api-key`
//! header, and the results in a `results` array on the way back.
//! Three shapes this adapter pins (vendor docs are the source of
//! truth, `specs/rhema-websearch.md` §4):
//!
//! - `contents.text` capped at [`TEXT_MAX_CHARACTERS`]: the seam's
//!   snippet is the page-text head, and an uncapped text is a whole
//!   page per hit.
//! - `category: "news"` carries the news flag; the search route is
//!   one route, the index choice is a body field.
//! - a 200 response whose `results` array is absent reads as zero
//!   results, not a parse failure: a vendor that answered with
//!   nothing is an honest answer, and a `Parse` error would bill the
//!   next provider in the chain for a query the first one settled.
//!
//! Layout: the [`wire`] types and the pure mapping (always compiled),
//! and the [`ExaProvider`] over reqwest (feature `native`).

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{DEFAULT_BASE_URL, ExaProvider, SEARCH_PATH, TEXT_MAX_CHARACTERS, USER_AGENT};

pub use wire::{ExaResponse, ExaResult, results_from_response};
