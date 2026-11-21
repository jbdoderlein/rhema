//! rhema-prompt, the system prompt: the Jinja template renderer and
//! the skills list it renders.
//!
//! - [`prompt`]: the template renderer: [`render_system`] for a system
//!   prompt, [`render_template`] for a caller's own variables
//!
//! Discovery of the skills themselves lives in `rhema-skills`; this
//! crate renders the discovered list into the template's `skills`
//! section.
#![forbid(unsafe_code)]

pub mod prompt;

pub use prompt::{Prompt, render_system, render_template};
// Re-exported so a caller can name the error the renderer returns
// without depending on minijinja itself.
pub use prompt::TemplateError;
// The prompt API takes a list of skills; the type lives in
// rhema-skills and rides along here.
pub use rhema_skills::Skill;
