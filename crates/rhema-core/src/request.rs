//! What goes to the model: the assembled request and its parameters.

use serde::{Deserialize, Serialize};

use crate::message::{Content, Message, Role};

/// A tool advertised to the model this step, straight from
/// [`crate::ToolResolver::specs`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Name the model uses to call the tool.
    pub name: String,
    /// What the tool does; the model picks tools from this.
    pub description: String,
    /// JSON schema for the call arguments.
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    /// A spec with a JSON-schema object of no required properties.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// How much reasoning the model should do, mapped by each provider onto
/// its own request parameter, OpenAI-compatible `reasoning_effort`,
/// OpenRouter's `reasoning.effort`, Anthropic's thinking budget, Gemini's
/// thinking budget. [`ThinkingEffort::Default`] sends nothing: the
/// server's own default stands, which keeps strict endpoints happy and
/// leaves always-thinking models as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    /// The provider decides; rhema sends no effort parameter. An explicit
    /// choice (`thinking: default`, `/thinking default`), the frontends
    /// resolve an unset level to [`ThinkingEffort::Medium`] instead.
    #[default]
    Default,
    /// Light reasoning, quick turns, cheap models.
    Low,
    /// The middle setting.
    Medium,
    /// Maximum reasoning effort.
    High,
}

impl ThinkingEffort {
    /// The level's name as the config, the flag, and the command spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Parses a level name; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "default" => Some(Self::Default),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Sampling parameters for the model call.
///
/// [`Self::top_p`] and [`Self::stop`] are best-effort, like
/// [`Self::temperature`] and [`Self::max_tokens`]: every adapter with a
/// wire field maps them, and an adapter whose API has no equivalent
/// ignores them rather than failing the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChatParams {
    /// Model identifier, provider-interpreted (e.g. `gemini-2.5-flash`).
    pub model: String,
    /// Sampling temperature, if the driver wants one.
    pub temperature: Option<f32>,
    /// Nucleus-sampling threshold in `0.0..=1.0`, if the driver wants
    /// one: the model samples only from the smallest set of tokens whose
    /// probabilities sum to this value. `None` leaves the provider's
    /// default; lower values make the output more predictable.
    pub top_p: Option<f32>,
    /// Output token ceiling, if the driver wants one.
    pub max_tokens: Option<u32>,
    /// Strings that end generation when the model would emit one. The
    /// stop text never appears in the reply. Empty leaves the provider's
    /// default; providers cap how many they accept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// How much reasoning the model should do. Defaults to
    /// [`ThinkingEffort::Default`] so pre-seam payloads load unchanged.
    #[serde(default)]
    pub thinking: ThinkingEffort,
}

impl ChatParams {
    /// Parameters naming `model`, with defaults for everything else.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }
}

/// The full request handed to a [`crate::ModelProvider`].
///
/// The engine hands providers a normalized request: [`ModelRequest::system`]
/// holds the whole system prompt, and [`ModelRequest::messages`] contains
/// no [`Role::System`] entries. Call [`ModelRequest::normalized`] when
/// building a request by hand, and every provider sees the same prompt
/// regardless of which adapter serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// System prompt, if the agent has one.
    pub system: Option<String>,
    /// The conversation so far, cloned from the session at step start.
    pub messages: Vec<Message>,
    /// Tools offered this step.
    pub tools: Vec<ToolSpec>,
    /// Sampling parameters.
    pub params: ChatParams,
}

impl ModelRequest {
    /// Folds every [`Role::System`] message into [`Self::system`] and
    /// removes it from [`Self::messages`]. The request's own system
    /// prompt comes first, then each system message's text in history
    /// order, joined with a blank line. Empty text contributes nothing,
    /// and non-text blocks in a system message are dropped.
    ///
    /// Idempotent. The engine normalizes every request before handing it
    /// to a provider, so an adapter can rely on the invariant; a caller
    /// that builds a [`ModelRequest`] itself should call this too.
    pub fn normalized(mut self) -> Self {
        let mut parts: Vec<String> = Vec::new();
        if let Some(system) = self.system.take()
            && !system.trim().is_empty()
        {
            parts.push(system);
        }
        self.messages = std::mem::take(&mut self.messages)
            .into_iter()
            .filter_map(|message| {
                if message.role != Role::System {
                    return Some(message);
                }
                for content in message.content {
                    if let Content::Text(text) = content
                        && !text.trim().is_empty()
                    {
                        parts.push(text);
                    }
                }
                None
            })
            .collect();
        self.system = (!parts.is_empty()).then(|| parts.join("\n\n"));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_round_trip_through_serde() {
        assert_eq!(ThinkingEffort::parse(" High "), Some(ThinkingEffort::High));
        assert_eq!(ThinkingEffort::parse("low"), Some(ThinkingEffort::Low));
        assert_eq!(ThinkingEffort::parse("maximal"), None);
        assert_eq!(ThinkingEffort::default(), ThinkingEffort::Default);

        let params = ChatParams {
            thinking: ThinkingEffort::High,
            ..ChatParams::new("m")
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"thinking\":\"high\""), "{json}");
        assert_eq!(serde_json::from_str::<ChatParams>(&json).unwrap(), params);
        // A payload from before the field existed loads as Default.
        let legacy: ChatParams = serde_json::from_str(r#"{"model":"m"}"#).unwrap();
        assert_eq!(legacy.thinking, ThinkingEffort::Default);
    }

    #[test]
    fn normalized_folds_system_messages_in_order_and_is_idempotent() {
        let request = ModelRequest {
            system: Some("base".to_owned()),
            messages: vec![
                Message::text(Role::System, "one"),
                Message::text(Role::User, "hi"),
                Message::text(Role::System, "two"),
            ],
            tools: Vec::new(),
            params: ChatParams::new("m"),
        };
        let normalized = request.clone().normalized();
        assert_eq!(normalized.system.as_deref(), Some("base\n\none\n\ntwo"));
        assert_eq!(normalized.messages.len(), 1);
        assert_eq!(normalized.messages[0].role, Role::User);
        assert_eq!(normalized.clone().normalized(), normalized);
    }

    #[test]
    fn normalized_drops_empty_system_messages_and_keeps_none() {
        let request = ModelRequest {
            system: Some("   ".to_owned()),
            messages: vec![
                Message::text(Role::System, "  "),
                Message::text(Role::User, "hi"),
            ],
            tools: Vec::new(),
            params: ChatParams::new("m"),
        };
        let normalized = request.normalized();
        assert_eq!(normalized.system, None);
        assert_eq!(normalized.messages.len(), 1);
    }

    #[test]
    fn sampling_fields_round_trip() {
        let params = ChatParams {
            model: "m".into(),
            top_p: Some(0.5),
            stop: vec!["STOP".into()],
            ..ChatParams::default()
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"top_p\":0.5"), "{json}");
        assert!(json.contains("\"stop\":[\"STOP\"]"), "{json}");
        assert_eq!(serde_json::from_str::<ChatParams>(&json).unwrap(), params);
    }

    #[test]
    fn new_sampling_fields_load_from_old_payloads() {
        let params: ChatParams =
            serde_json::from_str(r#"{"model":"m","temperature":0.5}"#).unwrap();
        assert_eq!(params.temperature, Some(0.5));
        assert_eq!(params.top_p, None);
        assert!(params.stop.is_empty());
    }
}
