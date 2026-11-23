//! The TypeSafe Jev adapter: one vendor's answer to the
//! [`DecisionProvider`](rhema_decision::DecisionProvider) seam.
//!
//! Jev's API is one route, `POST /v1/systemone`, the key in an
//! `Authorization: Bearer` header, and the request is the System One
//! body proper: the seam's request plus a `model` field (`jev-latest`
//! by default, the moving alias; a pinned `jev-1.13.0` for runs that
//! compare). The response is the seam's response as it stands, no
//! envelope. Two shapes this adapter pins (vendor docs are the source
//! of truth, `specs/rhema-decision.md` §4):
//!
//! - a 400 or 422 is [`Invalid`]: the vendor refused the request
//!   itself, and its `detail` (a sentence, or a validation error's
//!   `msg` entries) is the message the model reads.
//! - a 200 whose body is not the documented shape is [`Parse`], never
//!   a silent empty answer.
//!
//! Layout: the [`wire`] types and the pure shaping (always compiled),
//! and the [`JevProvider`] over reqwest (feature `native`).
//!
//! [`Invalid`]: rhema_decision::DecisionError::Invalid
//! [`Parse`]: rhema_decision::DecisionError::Parse

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{DEFAULT_BASE_URL, JevProvider, SYSTEMONE_PATH, USER_AGENT};

pub use wire::{DEFAULT_MODEL, request_body, response_from_json};
