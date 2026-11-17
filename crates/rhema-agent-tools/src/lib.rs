//! rhema-agent-tools, the split toolkit's old import path.
//!
//! The crate split in two: [`rhema_bash`] holds the approval-gated
//! shell and the macros that ride it (`see`, `job`, `search`), and
//! [`rhema_prompt`] holds the system-prompt renderer and the skill
//! discovery that feeds its skills list. This crate re-exports both, so
//! a consumer on the 0.3 names (`rhema_agent_tools::ShellTools`,
//! `rhema_agent_tools::prompt::render_system`, the `jobs` and `vision`
//! modules) keeps compiling. New code should depend on the two crates
//! directly.
#![forbid(unsafe_code)]

pub use rhema_bash::*;
pub use rhema_prompt::*;
