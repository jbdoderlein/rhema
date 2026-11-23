//! `rhema-decision`, the vocabulary of rhema decision models and the
//! one call made with it.
//!
//! A decision model (a System One model: TypeSafe's Jev, Cloudflare's
//! Clef) reads a state and typed questions and answers each with a
//! calibrated probability distribution in one forward pass. It
//! generates no text. Every `rhema-decision-*` crate implements the
//! [`DecisionProvider`] seam against one vendor's API; this crate
//! holds only what they all share: the [`DecisionRequest`] a caller
//! builds (a JSON state plus named questions of three kinds, `noul`,
//! `choice`, `score`), the [`DecisionResponse`] an adapter answers,
//! the [`DecisionError`] classes an adapter reports, and [`decide`],
//! the validated and budgeted call. No HTTP here: the transport stays
//! in the adapters, reqwest behind a `native` feature each, the
//! `rhema-websearch` arrangement.
//!
//! Both vendors speak the same wire, `/v1/systemone`: a request is
//! `{"model", "state", "questions"}` and a response is
//! `{"model", "answers", "usage"}`, answers keyed by the question
//! names the request chose. The vocabulary here serializes to exactly
//! that shape, so an adapter's request is the request plus its model
//! name, and its response is this crate's response parsed back.
//!
//! One profile asks one provider and one model, the pair the config
//! names; there is no fallback from one vendor to another, a failure
//! is the failure the model reads. The request is validated before
//! the vendor is asked, so a malformed one costs no call. Spec:
//! `specs/rhema-decision.md` §5.
//!
//! The macro that reaches the model (the `decide` bash function) is
//! the frontend's wiring (`rhema-bash::decision` plus the
//! `__decision` mode of the reference frontend); the claw and the
//! cloud harness assemble their own agents and hold a provider behind
//! the trait object, which is the whole point of the seam.
//!
//! Layout:
//! - [`question`]: the three question kinds
//! - [`request`]: the state plus the named questions
//! - [`answer`]: the three answer kinds
//! - [`response`]: the answers plus the model and the usage
//! - [`error`]: the failure classes
//! - [`provider`]: the adapter seam
//! - [`call`]: the validated, budgeted call
//! - [`mod@render`]: the text a `decide` command prints
//! - [`mock`]: the scripted provider (feature `mock`)

#![forbid(unsafe_code)]

pub mod answer;
pub mod call;
pub mod error;
pub mod provider;
pub mod question;
pub mod render;
pub mod request;
pub mod response;

#[cfg(feature = "mock")]
pub mod mock;

pub use answer::Answer;
pub use call::{DEFAULT_TIMEOUT, decide, decide_within};
pub use error::DecisionError;
pub use provider::DecisionProvider;
pub use question::{NoulCriteria, Question};
pub use render::{render, render_json};
pub use request::DecisionRequest;
pub use response::{DecisionResponse, Usage};
