//! The system-prompt template renderer: one markdown template, Jinja.
//!
//! The template language is Jinja, which prompt templates across the
//! LLM ecosystem already speak. [`render_system`] takes a [`Prompt`]:
//! the workspace, the discovered `skills`, the advertised `tools`, and
//! whatever named sections the caller's template references. Nothing
//! here knows a section's name, so a frontend's own macro arrives as
//! one more entry, not a new parameter. [`render_template`] is the same
//! engine over a plain context, for text that is nobody's system prompt
//! (the environment report's manifest, say): one syntax, one set of
//! whitespace rules. Rendering is fallible on purpose: a template error
//! surfaces loudly at agent construction instead of silently rendering
//! half the prompt, while a misspelled variable renders empty
//! (minijinja's lenient undefined).

use std::path::Path;

use minijinja::{Environment, Value};
use rhema_core::ToolSpec;

use rhema_skills::Skill;

/// The template engine's error, re-exported under a crate-local name:
/// [`render_system`] and [`render_template`] return it, and a caller
/// should not need a minijinja dependency to name it.
pub use minijinja::Error as TemplateError;

/// One render's inputs: the three values the driver produces, and the
/// named sections the caller's template references.
///
/// The three are the driver's own: it knows the workspace, it found the
/// skills, it advertises the tools. Sections are the caller's, and
/// nothing here names them. A frontend wiring a macro of its own (a
/// `hear` function beside `see`, say) contributes its section under the
/// variable its template spells, and the renderer places it exactly as
/// it places `environment` or `vision`. No section, no value: a template
/// that guards on it renders nothing, the same as an undefined variable.
#[derive(Debug)]
pub struct Prompt<'a> {
    workspace: &'a Path,
    skills: &'a [Skill],
    tools: &'a [ToolSpec],
    sections: Vec<(&'a str, serde_json::Value)>,
}

impl<'a> Prompt<'a> {
    /// The three the renderer always carries: `workspace` (the path as
    /// text), `skills` (the discovered list, `name`, `description`,
    /// `license`, `path`), and `tools` (the advertised specs, `name`,
    /// `description`, `parameters`, the JSON schema).
    pub fn new(workspace: &'a Path, skills: &'a [Skill], tools: &'a [ToolSpec]) -> Self {
        Self {
            workspace,
            skills,
            tools,
            sections: Vec::new(),
        }
    }

    /// Adds `value` under `name`, the template variable it renders as.
    /// Anything that is JSON can ride: a section's prose, a list of
    /// rows, a struct behind `serde_json::json!`. A later value of the
    /// same name replaces an earlier one.
    #[must_use]
    pub fn section(mut self, name: &'a str, value: impl Into<serde_json::Value>) -> Self {
        self.sections.push((name, value.into()));
        self
    }

    /// Adds a whole list at once, the shape a frontend folds its config
    /// into. Same naming rule as [`section`](Self::section).
    #[must_use]
    pub fn with_sections(
        mut self,
        sections: impl IntoIterator<Item = (&'a str, serde_json::Value)>,
    ) -> Self {
        self.sections.extend(sections);
        self
    }
}

/// Renders a SYSTEM.md template over a [`Prompt`]: `{{ workspace }}`
/// becomes the workspace path, `skills` and `tools` are loops over
/// their lists, and every section the prompt carries is a variable of
/// its name. Undefined variables render empty; syntax and filter errors
/// return with template positions. `{{ tools[i].parameters | json }}`
/// is the compact spelling of a schema; `tojson` is the HTML-safe one.
///
/// This is [`render_template`] over `Prompt`'s variables. A caller with
/// none of them (a report, a document) calls [`render_template`]
/// directly with its own context.
pub fn render_system(template: &str, prompt: &Prompt<'_>) -> Result<String, TemplateError> {
    let mut context = serde_json::Map::new();
    context.insert(
        "workspace".to_owned(),
        serde_json::json!(prompt.workspace.display().to_string()),
    );
    context.insert("skills".to_owned(), serde_json::json!(prompt.skills));
    context.insert("tools".to_owned(), serde_json::json!(prompt.tools));
    for (name, value) in &prompt.sections {
        context.insert((*name).to_owned(), value.clone());
    }
    render_template(template, &serde_json::Value::Object(context))
}

/// The `json` filter: a value as compact JSON, no HTML-safe escaping.
fn json(value: Value) -> Result<String, TemplateError> {
    serde_json::to_string(&value).map_err(|err| {
        TemplateError::new(
            minijinja::ErrorKind::InvalidOperation,
            "cannot serialize to JSON",
        )
        .with_source(err)
    })
}

/// Renders `template` over `context`, the same engine and whitespace
/// rules as [`render_system`], with the caller's own variables. A
/// value's own text is inserted verbatim, never re-parsed, so a
/// section's braces survive; the template name in errors is `system`.
pub fn render_template(
    template: &str,
    context: &serde_json::Value,
) -> Result<String, TemplateError> {
    let mut env = Environment::new();
    // Block tags may sit on their own lines in a markdown template
    // without blank-line runoff.
    env.set_syntax(
        minijinja::syntax::SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()?,
    );
    // `json` is `tojson` without the HTML-safe escaping: `tojson`
    // escapes `<`, `>`, `&`, and `'` as `\u003c`-style sequences so the
    // output can ride a script tag, and a prompt's schema column is not
    // HTML, the escapes cost tokens and reach the model as
    // `this session\u0027s`. The names are distinct so a user template
    // that wants the HTML-safe spelling keeps it.
    env.add_filter("json", json);
    env.add_template("system", template)?;
    Ok(env
        .get_template("system")?
        .render(Value::from(minijinja::value::Serde(context)))?
        .trim_end()
        .to_owned()
        + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn skill(name: &str, description: &str) -> Skill {
        Skill {
            name: name.into(),
            description: description.into(),
            license: None,
            path: format!("/cfg/skills/{name}/SKILL.md").into(),
            body: "the rules".into(),
        }
    }

    fn tool(name: &str, description: &str) -> ToolSpec {
        ToolSpec::new(
            name,
            description,
            json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }),
        )
    }

    /// The four-argument shape most tests use: a prompt with no
    /// sections.
    fn render(
        template: &str,
        workspace: &Path,
        skills: &[Skill],
        tools: &[ToolSpec],
    ) -> Result<String, TemplateError> {
        render_system(template, &Prompt::new(workspace, skills, tools))
    }

    #[test]
    fn render_fills_the_workspace_and_leaves_other_braces_alone() {
        let out = render(
            "work in {{ workspace }}, keep {mystery}",
            Path::new("/ws"),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(out, "work in /ws, keep {mystery}\n");
    }

    #[test]
    fn a_for_loop_renders_its_body_once_per_skill() {
        let out = render(
            "{% for skill in skills %}- {{ skill.name }}: {{ skill.description }}, cat {{ skill.path }}\n{% endfor %}",
            Path::new("/ws"),
            &[skill("a", "da"), skill("b", "db")],
            &[],
        )
        .unwrap();
        assert_eq!(
            out,
            "- a: da, cat /cfg/skills/a/SKILL.md\n- b: db, cat /cfg/skills/b/SKILL.md\n"
        );
    }

    #[test]
    fn a_for_loop_renders_its_body_once_per_tool_with_the_schema() {
        let out = render(
            "{% for tool in tools %}- {{ tool.name }}: {{ tool.description }}, {{ tool.parameters | tojson }}\n{% endfor %}",
            Path::new("/ws"),
            &[],
            &[tool("t", "dt")],
        )
        .unwrap();
        assert_eq!(
            out,
            "- t: dt, {\"properties\": {\"command\": {\"type\": \"string\"}}, \"required\": [\"command\"], \"type\": \"object\"}\n"
        );
    }

    /// The `json` filter is `tojson` without the HTML-safe escapes: a
    /// prompt's schema column carries `'` and `<` as themselves rather
    /// than as `\u0027` and `\u003c`, and a template that embeds its
    /// output in a page can still ask for the escaped spelling.
    #[test]
    fn the_json_filter_leaves_html_characters_alone() {
        let spec = ToolSpec::new(
            "t",
            "the session's tool",
            json!({
                "type": "object",
                "properties": {
                    "model": { "type": "string", "description": "a session's <provider>/<model>" }
                }
            }),
        );
        let out = render(
            "{{ tools[0].parameters | json }}",
            Path::new("/ws"),
            &[],
            std::slice::from_ref(&spec),
        )
        .unwrap();
        assert!(out.contains(r#""a session's <provider>/<model>""#), "{out}");
        assert!(!out.contains("\\u0027"), "{out}");
        assert!(!out.contains("\\u003c"), "{out}");

        let escaped = render(
            "{{ tools[0].parameters | tojson }}",
            Path::new("/ws"),
            &[],
            &[spec],
        )
        .unwrap();
        assert!(escaped.contains("\\u0027"), "{escaped}");
    }

    #[test]
    fn the_mcp_section_renders_lines_and_vanishes_when_empty() {
        let template = "{% if mcp %}MCP:\n{% for server in mcp %}- {{ server.name }} (server: {{ server.server }}): {{ server.description }}\n{% endfor %}{% endif %}";
        // Any list of rows renders; naming the type is not this
        // crate's business, a literal stands in.
        let lines = serde_json::json!([{
            "name": "atlassian",
            "server": "atlassian",
            "description": "Jira and Confluence",
        }]);
        let with = Prompt::new(Path::new("/ws"), &[], &[]).section("mcp", lines);
        let out = render_system(template, &with).unwrap();
        assert_eq!(
            out,
            "MCP:\n- atlassian (server: atlassian): Jira and Confluence\n"
        );
        // No rows, no section: the empty list is falsy in the template,
        // the same render a prompt that never carried one produces.
        let empty = Prompt::new(Path::new("/ws"), &[], &[]).section("mcp", json!([]));
        let out = render_system(template, &empty).unwrap();
        assert!(!out.contains("atlassian"), "{out:?}");
        assert!(!out.contains("MCP:"), "{out:?}");
    }

    #[test]
    fn an_if_around_the_skills_section_holds_a_vanishing_heading() {
        let template = "{% if skills %}Skills (name / trigger / file):\n{% for skill in skills %}{{ skill.name }} / {{ skill.description }} / {{ skill.path }}\n{% endfor %}{% endif %}";
        let with = render(template, Path::new("/ws"), &[skill("s", "d")], &[]).unwrap();
        assert_eq!(
            with,
            "Skills (name / trigger / file):\ns / d / /cfg/skills/s/SKILL.md\n"
        );
        let without = render(template, Path::new("/ws"), &[], &[]).unwrap();
        assert_eq!(without, "\n");
    }

    #[test]
    fn a_misspelled_variable_renders_empty_not_an_error() {
        let out = render(
            "{% for skill in skills %}{{ skill.nope }}{{ skill.name }}{% endfor %}",
            Path::new("/ws"),
            &[skill("s", "d")],
            &[],
        )
        .unwrap();
        assert_eq!(out, "s\n");
    }

    #[test]
    fn a_template_error_surfaces_instead_of_rendering_prose() {
        let err = render("{% if skills %} never closed", Path::new("/ws"), &[], &[])
            .expect_err("an unclosed block is a loud error, not prose");
        assert!(err.to_string().contains("system"), "{err}");
    }

    #[test]
    fn a_template_without_the_block_presents_no_skills() {
        let out = render(
            "base in {{ workspace }}\n",
            Path::new("/ws"),
            &[skill("s", "d")],
            &[],
        )
        .unwrap();
        assert_eq!(out, "base in /ws\n");
    }

    #[test]
    fn the_sections_render_where_the_template_places_them() {
        let template = "open\n{% if user %}{{ user }}\n{% endif %}{% if environment %}{{ environment }}\n{% endif %}close";
        let prompt = Prompt::new(Path::new("/ws"), &[], &[])
            .section("environment", "- OS: linux")
            .section("user", "prefer short answers");
        let out = render_system(template, &prompt).unwrap();
        assert_eq!(out, "open\nprefer short answers\n- OS: linux\nclose\n");
    }

    /// The reason sections are a map and not parameters: a frontend's
    /// own macro (an audio `hear` beside the image `see`, say) names
    /// its own variable, and the template decides where it lands. No
    /// list of names here has to grow for it.
    #[test]
    fn a_section_the_crate_never_names_renders_where_the_template_places_it() {
        let prompt = Prompt::new(Path::new("/ws"), &[], &[])
            .section("hear", "`hear clip.mp3` attaches the clip as audio");
        let out = render_system("{% if hear %}Audio:\n{{ hear }}\n{% endif %}", &prompt).unwrap();
        assert_eq!(out, "Audio:\n`hear clip.mp3` attaches the clip as audio\n");
    }

    #[test]
    fn with_sections_takes_a_frontend_s_folded_list() {
        let sections = vec![("vision", json!("see docs")), ("user", json!("terse"))];
        let prompt = Prompt::new(Path::new("/ws"), &[], &[]).with_sections(sections);
        let out = render_system("{{ vision }} / {{ user }}", &prompt).unwrap();
        assert_eq!(out, "see docs / terse\n");
    }

    #[test]
    fn a_later_section_of_the_same_name_replaces_the_earlier_one() {
        let prompt = Prompt::new(Path::new("/ws"), &[], &[])
            .section("jobs", "first")
            .section("jobs", "second");
        assert_eq!(render_system("{{ jobs }}", &prompt).unwrap(), "second\n");
    }

    #[test]
    fn absent_sections_render_nothing_like_undefined_variables() {
        let bare = render(
            "open\n{% if user %}{{ user }}\n{% endif %}close",
            Path::new("/ws"),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(bare, "open\nclose\n");
    }

    #[test]
    fn render_template_takes_the_callers_own_variables() {
        let out = render_template(
            "hi {{ who }}{% if tag %} ({{ tag }}){% endif %}",
            &json!({ "who": "claw", "tag": "" }),
        )
        .unwrap();
        assert_eq!(out, "hi claw\n");
    }

    #[test]
    fn a_values_own_text_is_inserted_verbatim_never_reparsed() {
        let out = render_template(
            "{{ section }}",
            &json!({ "section": "- braces stay: {{ keep }} and {soul}\n" }),
        )
        .unwrap();
        assert_eq!(out, "- braces stay: {{ keep }} and {soul}\n");
    }
}
