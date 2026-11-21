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
// The prompt API takes a list of skills; the types live in
// rhema-skills and ride along here, SkillTier so a caller building
// skills by hand can stamp one without a rhema-skills dependency.
pub use rhema_skills::{Skill, SkillTier};
