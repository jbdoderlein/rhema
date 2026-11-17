//! rhema-coding-agent, the coding-agent driver, the first consumer of
//! rhema-core's step engine.
//!
//! Wires a [`ModelProvider`](rhema_core::ModelProvider) into a
//! [`CodingAgent`] whose turns drive `rhema_core::run_turn` over the
//! tools (`rhema-bash` and `rhema-prompt`): the approval-gated shell,
//! its spec and discovered skills presented in the system prompt, the
//! model loading skill bodies via the shell. Tool errors surface in-band to
//! the model (SPEC.md §4); approval is config state, installed into the
//! tools before each turn, and a driver whose channel belongs to one
//! turn swaps it with [`CodingAgent::set_approval`]; the session is
//! plain data the driver can persist and resume, with a commit hook so
//! an intent is durable before its first side effect, revive-on-resume
//! for calls a crash left unanswered, and cancel-path repair for those
//! a user stopped.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use rhema_coding_agent::{AllowAll, CodingAgent, CodingAgentConfig};
//! use rhema_core::{Event, EventSink, ModelProvider};
//!
//! /// A headless sink: the events are dropped.
//! struct Quiet;
//! impl EventSink for Quiet {
//!     fn emit(&mut self, _event: Event<'_>) {}
//! }
//!
//! # async fn example(provider: impl ModelProvider + 'static) -> Result<(), Box<dyn std::error::Error>> {
//! let config = CodingAgentConfig::new(
//!     provider,
//!     "claude-sonnet-5-5",
//!     "/work/repo",
//!     AllowAll, // any Approval; DenyAll refuses every call
//! );
//! let mut agent = CodingAgent::new(config)?;
//!
//! let _reply = agent
//!     .turn("fix the failing test", &mut Quiet)
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Any [`EventSink`](rhema_core::EventSink) works: the runner's channel,
//! a UI's redraw hook, or a headless sink that drops the stream. The provider and the approval both arrive as `impl Trait`
//! and are wrapped internally, so a concrete provider and an `Arc` the
//! caller already holds pass equally unchanged.
//!
//! Layout:
//! - [`agent`]: `CodingAgent`, `CodingAgentError`, `stamped_system`
//! - [`config`]: `CodingAgentConfig` and the `Steering` seam
//! - [`prompt`]: the SYSTEM.md template and its default render
//! - [`environment`]: the startup environment report and the sandbox
//!   notice, the prompt's environment section
//!
//! The tools themselves, the `Approval` seam, `ShellTools`, skills, the
//! renderer, live in `rhema-bash` and `rhema-prompt` and are re-exported
//! here: the TUI and ACP drivers import these names from this crate, and
//! the claw's forge handler takes the approval policy the same way.
//!
//! # Skills
//!
//! A skill is a folder with a `SKILL.md` whose frontmatter carries a
//! `description` (the trigger). Only name, trigger, and file enter the
//! system prompt, as the [`prompt`] module's `skills` list; the model
//! reads a body itself through the bash tool when a trigger applies.
//! Skills live per-workspace under `.rhema/skills`, globally under the
//! config directory, and, at the lowest tier, in the cross-agent
//! `~/.agents/skills`. A name claimed by a higher tier hides the lower
//! copies, project over global over `.agents`.

#![forbid(unsafe_code)]

pub mod agent;
pub mod config;
pub mod environment;
pub mod prompt;

pub use agent::{
    CodingAgent, CodingAgentError, clear_environment_stamp, stamped_system,
    stamped_system_with_sections, stamped_system_with_skills,
};
pub use config::{CodingAgentConfig, Steering, SteeringChain, SubagentProviders};
pub use environment::{host_environment, host_report, remote_notice};
pub use prompt::{DEFAULT_SYSTEM_MD, default_system};
// The two tool crates the driver is built on, under this crate's name:
// every downstream driver (TUI, ACP, the claw) keeps its import paths.
pub use rhema_bash::{AllowAll, Approval, DenyAll, ShellTools};
pub use rhema_prompt::{Prompt, render_system, render_template};
