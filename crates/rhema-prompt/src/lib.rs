//! rhema-prompt, the system prompt: the Jinja template renderer and the
//! skills list it renders.
//!
//! - [`prompt`]: the template renderer: [`render_system`] for a system
//!   prompt, [`render_template`] for a caller's own variables
//! - [`skills`]: discovery of markdown skills, prompt presentation
//!
//! # Skills
//!
//! A skill is a folder with a `SKILL.md` whose frontmatter carries a
//! `description` (the trigger). Only name, trigger, and license enter
//! the system prompt; the model reads a body itself through the bash
//! tool when a trigger applies, so a skill needs no tool of its own. A
//! skill lives globally under the config directory and per-workspace
//! under `.rhema/skills`, project shadowing global.
#![forbid(unsafe_code)]

pub mod prompt;
pub mod skills;

pub use prompt::{Prompt, render_system, render_template};
// Re-exported so a caller can name the error the renderer returns
// without depending on minijinja itself.
pub use prompt::TemplateError;
pub use skills::{MAX_SKILL_BYTES, Skill, discover};
