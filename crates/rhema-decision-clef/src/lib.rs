//! The Cloudflare Clef adapter: one vendor's answer to the
//! [`DecisionProvider`](rhema_decision::DecisionProvider) seam.
//!
//! Clef is hosted on Workers AI, which serves the System One body on
//! its model route: `POST /client/v4/accounts/{account_id}/ai/run/
//! @cf/cloudflare/clef` (or `clef-flash`), the token in an
//! `Authorization: Bearer` header. Two things differ from Jev, and
//! this adapter owns both (vendor docs are the source of truth,
//! `specs/rhema-decision.md` §4):
//!
//! - the model picks the route. `clef` is the 27B model, `clef-flash`
//!   the 9B one, each its own route; the body's `model` field repeats
//!   the choice, as the route requires.
//! - the answer rides Workers AI's envelope, `{"result": {...},
//!   "success": true, "errors": [], "messages": []}`, with the System
//!   One response under `result` and a failure's reasons under
//!   `errors[].message`.
//!
//! Layout: the [`wire`] types and the pure shaping (always compiled),
//! and the [`ClefProvider`] over reqwest (feature `native`).

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{ClefProvider, DEFAULT_BASE_URL, USER_AGENT};

pub use wire::{ClefModel, route_path, unwrap_envelope};
