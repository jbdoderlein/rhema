//! `rhema-provider-mistral` adapts the Mistral API and Mistral Vibe to
//! rhema-core's [`ModelProvider`](rhema_core::provider::ModelProvider)
//! seam.
//!
//! Mistral's hosted API speaks two request shapes: OpenAI-compatible
//! chat completions and the Responses API beside it, one
//! `POST {base}/chat/completions` or `POST {base}/responses` with
//! `stream: true` per exchange, the API key in the `Authorization:
//! Bearer` header, and the SSE stream decoded by the shared
//! OpenAI-compatible core; [`MistralProvider::with_api`]
//! chooses between them. The same routes serve a Mistral Vibe
//! subscription, because Vibe's browser sign-in mints an ordinary
//! Mistral API key scoped to the signed-in plan.
//!
//! `GET {base}/models` names the chat models plus their context windows
//! and vision capability; a Vibe subscription reads
//! `GET {base}/model-catalog?surface=vibe-code` instead, falling back to
//! `/models` on deployments without it. The adapter adds the published
//! per-MTok prices from its own table.
//!
//! Two credentials reach the same routes:
//! - a Mistral API key, pasted into the login dialog;
//! - a Vibe subscription key, minted by [`MistralVibeLogin`] through
//!   `console.mistral.ai`'s PKCE browser sign-in.
//!
//! Layout:
//! - [`login`]: the browser sign-in that turns a Vibe subscription into
//!   an API key
//! - [`pricing`]: the curated per-MTok price table
//! - [`wire`]: the model page and its normalization into
//!   [`ModelCard`](rhema_provider::ModelCard) rows, plus the Vibe plan
//!   budget's [`Allowance`](rhema_provider::Allowance)
//! - [`provider`]: the transport, the catalog fetch, the reasoning
//!   effort clamp, and the
//!   [`ModelProvider`](rhema_core::provider::ModelProvider) implementation

#![forbid(unsafe_code)]

pub mod login;
pub mod pricing;
pub mod provider;
pub mod wire;

pub use login::{DEFAULT_AUTH_API_BASE_URL, MistralVibeLogin};
pub use pricing::model_pricing;
pub use provider::{DEFAULT_BASE_URL, MistralProvider, USER_AGENT};
pub use wire::{
    WireBudget, WireCapabilities, WireModel, WireModelsPage, allowance, normalize_models,
    reasoning_levels,
};
