//! The OpenRouter Decisions adapter: the aggregator's answer to the
//! [`DecisionProvider`](rhema_decision::DecisionProvider) seam.
//!
//! OpenRouter serves the System One models behind one route,
//! `POST /api/alpha/decisions`: the key in an `Authorization: Bearer`
//! header, the request the System One body with the model as a slug
//! (`typesafe/jev-1.14` by default, `cloudflare/clef-flash`,
//! `~typesafe/jev-latest`, any the catalog lists under
//! `output_modalities=decisions`, three of them free). Jev's own wire
//! under another spelling: the same body, the same answers, and a
//! per-question `{"type": "refusal"}` when a provider declines one
//! question and answers the rest. The route is alpha (the router's
//! docs say the shape may move): a change breaks this adapter's
//! `Parse` arm and nothing else (`specs/rhema-decision.md` §4).
//!
//! Layout: the [`wire`] types and the pure shaping (always compiled),
//! and the [`OpenRouterProvider`] over reqwest (feature `native`).

#![forbid(unsafe_code)]

pub mod wire;

#[cfg(feature = "native")]
pub mod provider;

#[cfg(feature = "native")]
pub use provider::{
    APP_REFERER, APP_TITLE, DECISIONS_PATH, DEFAULT_BASE_URL, OpenRouterProvider, USER_AGENT,
};

pub use wire::{DEFAULT_MODEL, request_body, response_from_json};
