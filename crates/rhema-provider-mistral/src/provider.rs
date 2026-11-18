//! The [`MistralProvider`]: Mistral's hosted chat completions and model
//! catalog.
//!
//! Completions ride the shared OpenAI-compatible core at
//! [`DEFAULT_BASE_URL`], so the tool-call, reasoning, and stop-reason
//! mapping stays one implementation. The catalog is fetched here
//! instead: Mistral's `/models` is a bare array with capability and
//! context metadata, and a Vibe subscription reads its own
//! `/model-catalog` envelope; both are richer than the generic
//! normalizer expects.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use rhema_core::event::EventSink;
use rhema_core::output::ModelOutput;
use rhema_core::provider::{ModelProvider, ProviderError};
use rhema_core::request::{ModelRequest, ThinkingEffort};
use rhema_provider::{Allowance, ModelCard, ModelListing};
use rhema_provider_openai_compatible::provider::{OpenAiCompatibleProvider, network_error};
use rhema_provider_openai_compatible::{Dialect, reasoning_effort_value};

use crate::wire;
use crate::wire::{WireBudget, WireModelsPage, normalize_models, reasoning_levels};

/// The production Mistral API base, version segment included.
pub const DEFAULT_BASE_URL: &str = "https://api.mistral.ai/v1";

/// Mistral's deviations from the generic OpenAI shape. Its assistant
/// message schema rejects `reasoning_content` (`extra_forbidden`), so
/// the chain of thought is replayed as structured `thinking` chunks in
/// `content`; its stream always carries the final usage chunk, so
/// `stream_options` is not requested either.
const MISTRAL: Dialect = Dialect {
    args_key: "_mistral_args_text",
    assistant_reasoning_key: None,
    replays_reasoning_as_thinking: true,
    effort_key: "reasoning_effort",
    sends_stream_options: false,
    reasoning_effort: reasoning_effort_value,
};

/// The client identification Mistral's edge records instead of the HTTP
/// library's default.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// The HTTP client every instance uses: bounded connects and name
/// lookups, the client's own name, and a bounded catalog call.
pub fn http_client() -> reqwest::Client {
    rhema_provider::http::client_builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("the shared client builder is valid")
}

/// Which catalog route a provider asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogRoute {
    /// The public API's bare `/models` array.
    Models,
    /// Vibe's `model-catalog` envelope, which names subscription models.
    Vibe,
}

/// The Mistral API and Mistral Vibe, speaking the shared
/// OpenAI-compatible chat-completions core.
pub struct MistralProvider {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
    transport: OpenAiCompatibleProvider,
    catalog: CatalogRoute,
    /// The reasoning-effort levels each model accepts, fetched once on
    /// the first request that needs clamping. A failed fetch caches an
    /// empty map rather than retrying every turn: the catalog answers
    /// for the session or requests fly as written.
    reasoning: tokio::sync::OnceCell<HashMap<String, Vec<String>>>,
}

impl std::fmt::Debug for MistralProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key never prints; the plain data does.
        f.debug_struct("MistralProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("http", &self.http)
            .finish()
    }
}

impl MistralProvider {
    /// A provider authenticating with `api_key` against
    /// [`DEFAULT_BASE_URL`]. The same key serves the Mistral API and a
    /// Mistral Vibe subscription, whose browser sign-in mints it.
    ///
    /// An empty key is accepted but useless against the hosted API: the
    /// header is omitted and the API answers 401.
    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            transport: transport_for(DEFAULT_BASE_URL, &api_key),
            api_key,
            http: http_client(),
            catalog: CatalogRoute::Models,
            reasoning: tokio::sync::OnceCell::new(),
        }
    }

    /// A Vibe subscription: the same chat route, but its own catalog
    /// endpoint, because the subscription lists routed models the public
    /// `/models` page does not always carry.
    pub fn vibe(api_key: impl Into<String>) -> Self {
        Self {
            catalog: CatalogRoute::Vibe,
            ..Self::new(api_key)
        }
    }

    /// Targets `base_url`, the full versioned base used verbatim with
    /// `/chat/completions` and `/models` appended; trailing slashes are
    /// trimmed. This is how a stored instance reaches a dedicated
    /// deployment, and how tests reach a mock server.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self.transport = transport_for(&self.base_url, &self.api_key);
        self
    }

    /// The chat-capable models the policy can serve (`GET {base}/models`),
    /// with their context windows, vision capability, and published
    /// prices.
    async fn fetch_models(&self) -> Result<Vec<ModelCard>, ProviderError> {
        match self.catalog {
            CatalogRoute::Models => self.fetch_models_from("models").await,
            CatalogRoute::Vibe => {
                // Vibe's own catalog is the authoritative list for a
                // subscription; its `{"models":[...]}` body also names
                // routed models the public route may omit. A deployment
                // that does not serve it falls back to `/models`.
                match self.fetch_models_from("model-catalog?surface=vibe-code").await {
                    Ok(models) if !models.is_empty() => Ok(models),
                    Ok(_) => self.fetch_models_from("models").await,
                    Err(catalog_error) => {
                        self.fetch_models_from("models")
                            .await
                            .map_err(|models_error| ProviderError::Api {
                                message: format!(
                                    "vibe catalog failed ({catalog_error}); models fallback failed ({models_error})"
                                ),
                                status: None,
                            })
                    }
                }
            }
        }
    }

    /// Fetches one catalog route under the base and normalizes it.
    async fn fetch_models_from(&self, route: &str) -> Result<Vec<ModelCard>, ProviderError> {
        let page = self.fetch_page(route).await?;
        let mut models = normalize_models(page);
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    /// Fetches one catalog route under the base and parses it, leaving
    /// the page whole for the two readers of it: the model listing and
    /// the per-model reasoning levels.
    async fn fetch_page(&self, route: &str) -> Result<WireModelsPage, ProviderError> {
        let response = self
            .http
            .get(format!("{}/{route}", self.base_url))
            .bearer_auth_if_present(&self.api_key)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(api_error(status, &text));
        }
        // Read the body as text: a deployment answering an HTML error
        // page with a 200, or a page behind a proxy, must say what came
        // back instead of only "expected value at line 1 column 1".
        let body = response.text().await.map_err(network_error)?;
        let body = body.trim_start_matches('\u{feff}').trim();
        let page: WireModelsPage = serde_json::from_str(body).map_err(|err| {
            let head: String = body.chars().take(180).collect();
            ProviderError::Api {
                message: format!("malformed models page: {err}; response begins {head:?}"),
                status: None,
            }
        })?;
        Ok(page)
    }

    /// The reasoning-effort levels each model accepts, fetched once per
    /// provider from the same catalog the model listing reads. A page
    /// that fails or publishes no levels yields an empty map: requests
    /// then fly with the effort as chosen and the API's own rejection
    /// is the diagnosis the user sees.
    async fn reasoning_levels(&self) -> &HashMap<String, Vec<String>> {
        self.reasoning
            .get_or_init(|| async {
                let page = match self.catalog {
                    CatalogRoute::Models => self.fetch_page("models").await,
                    // Vibe's catalog is the one that publishes the levels;
                    // its models fallback keeps a broken route from costing
                    // more than the clamp.
                    CatalogRoute::Vibe => {
                        match self.fetch_page("model-catalog?surface=vibe-code").await {
                            Ok(page) => Ok(page),
                            Err(_) => self.fetch_page("models").await,
                        }
                    }
                };
                page.map(reasoning_levels).unwrap_or_default()
            })
            .await
    }

    /// The plan's monthly Vibe Code budget, what the footer's badge
    /// shows (`GET {base}/billing/budget`). `Ok(None)` when the key has
    /// no Vibe subscription or a dedicated deployment serves no
    /// billing routes: no badge, never a wrong figure. The catalog-less
    /// `mistral` provider key reads the same route when it carries a
    /// Vibe plan.
    ///
    /// # Errors
    ///
    /// Fails with [`ProviderError::Network`]/[`ProviderError::Timeout`]
    /// or [`ProviderError::Api`] on a non-2xx answer, like the other
    /// account fetches.
    pub async fn allowance(&self) -> Result<Option<Allowance>, ProviderError> {
        let response = self
            .http
            .get(format!("{}/billing/budget", self.base_url))
            .bearer_auth_if_present(&self.api_key)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            // The routes the badge never concludes anything from: a
            // rejected key, a member without billing sight, or a
            // deployment without the route. Other failures stay errors.
            if matches!(status.as_u16(), 401 | 403 | 404) {
                return Ok(None);
            }
            let text = response.text().await.unwrap_or_default();
            return Err(api_error(status, &text));
        }
        let budget: WireBudget = response.json().await.map_err(|err| ProviderError::Api {
            message: format!("malformed billing budget response: {err}"),
            status: None,
        })?;
        Ok(wire::allowance(&budget))
    }
}

/// Clamps an effort to the levels the model accepts. Mistral answers a
/// `reasoning_effort` the model does not list with a 400/401, and the
/// catalog tells each model's exhaustive set, so an unroutable level
/// moves to the nearest one the model speaks: `medium` becomes `high`
/// on models that jump, a `low` a reasoning-only-when-asked model
/// cannot honor becomes no parameter at all (its own default is no
/// thinking), and an exact fit flies as chosen. An empty set is no
/// catalog knowledge: the choice stands as made.
fn clamped_effort(thinking: ThinkingEffort, allowed: &[String]) -> ThinkingEffort {
    if allowed.is_empty() {
        return thinking;
    }
    let accepts = |level: &str| allowed.iter().any(|allowed| allowed == level);
    match thinking {
        ThinkingEffort::Default => ThinkingEffort::Default,
        ThinkingEffort::Low if !accepts("low") => ThinkingEffort::Default,
        ThinkingEffort::Medium if !accepts("medium") => {
            if accepts("high") {
                ThinkingEffort::High
            } else if accepts("low") {
                ThinkingEffort::Low
            } else {
                ThinkingEffort::Default
            }
        }
        ThinkingEffort::High if !accepts("high") => {
            if accepts("medium") {
                ThinkingEffort::Medium
            } else if accepts("low") {
                ThinkingEffort::Low
            } else {
                ThinkingEffort::Default
            }
        }
        chosen => chosen,
    }
}

/// Maps a non-2xx body onto [`ProviderError::Api`], keeping the
/// message Mistral publishes. Its errors are not uniformly OpenAI
/// shaped: a bad key returns `{"detail": "..."}`, a chat rejection an
/// `error.message`, and some routes a top-level `message`.
pub fn api_error(status: reqwest::StatusCode, raw: &str) -> ProviderError {
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
    let message = parsed
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .or_else(|| parsed.get("message").and_then(serde_json::Value::as_str))
        .or_else(|| {
            parsed
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or(raw);
    ProviderError::Api {
        message: format!("mistral api error (HTTP {}): {message}", status.as_u16()),
        status: Some(status.as_u16()),
    }
}

/// The dialect-core transport for one base: the client name, and the
/// bearer key when present.
fn transport_for(base_url: &str, api_key: &str) -> OpenAiCompatibleProvider {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(USER_AGENT),
    );
    OpenAiCompatibleProvider::new(api_key)
        .with_base_url(base_url)
        .with_dialect(MISTRAL)
        .with_extra_headers(headers)
}

/// Attaches the bearer header when a key is configured; an empty key
/// means an anonymous catalog request, which answers 401.
trait BearerIfPresent {
    fn bearer_auth_if_present(self, api_key: &str) -> Self;
}

impl BearerIfPresent for reqwest::RequestBuilder {
    fn bearer_auth_if_present(self, api_key: &str) -> Self {
        if api_key.is_empty() {
            self
        } else {
            self.bearer_auth(api_key)
        }
    }
}

#[async_trait]
impl ModelListing for MistralProvider {
    /// The account's chat models (`GET {base}/models`), normalized to
    /// [`ModelCard`] rows with their published context windows,
    /// vision capability, and per-MTok prices.
    async fn list_models(&self) -> Result<Vec<ModelCard>, ProviderError> {
        self.fetch_models().await
    }
}

#[async_trait]
impl ModelProvider for MistralProvider {
    /// # Errors
    ///
    /// Fails with [`ProviderError::Network`]/[`ProviderError::Timeout`]
    /// when the transport fails, [`ProviderError::Api`] with the HTTP
    /// status on a non-2xx response, and [`ProviderError::Api`] without
    /// one on a malformed stream chunk or a mid-stream error chunk.
    async fn complete(
        &self,
        req: ModelRequest,
        sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        let mut req = req;
        let chosen = req.params.thinking;
        if !matches!(chosen, ThinkingEffort::Default) {
            let levels = self.reasoning_levels().await;
            let allowed = levels
                .get(req.params.model.trim())
                .cloned()
                .unwrap_or_default();
            req.params.thinking = clamped_effort(chosen, &allowed);
        }
        self.transport.complete(req, sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_prints_presence_never_the_key() {
        let provider = MistralProvider::new("sk-live-secret");
        let printed = format!("{provider:?}");
        assert!(!printed.contains("sk-live-secret"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(printed.contains("base_url"), "{printed}");
    }

    #[test]
    fn the_error_message_reads_mistrals_shapes() {
        let status = reqwest::StatusCode::UNAUTHORIZED;
        let detail = api_error(status, r#"{"detail":"Invalid API Key"}"#);
        assert!(detail.to_string().contains("Invalid API Key"), "{detail}");
        let chat = api_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"model refused"}}"#,
        );
        assert!(chat.to_string().contains("model refused"), "{chat}");
        let raw = api_error(status, "not json");
        assert!(raw.to_string().contains("not json"), "{raw}");
    }

    #[test]
    fn the_mistral_dialect_never_sends_reasoning_content() {
        use rhema_core::request::ThinkingEffort;
        use rhema_core::{ChatParams, Content, Message, ModelRequest, Role, ToolCall, ToolResult};
        use rhema_provider_openai_compatible::wire::request_body;
        use serde_json::json;

        let req = ModelRequest {
            system: None,
            messages: vec![
                Message {
                    annotations: Default::default(),
                    role: Role::Assistant,
                    content: vec![
                        Content::Text("checking".into()),
                        Content::ToolCall(ToolCall {
                            id: "call_0".into(),
                            name: "echo".into(),
                            args: json!({ "x": 1 }),
                            provider_data: json!({ "reasoning": "because" }),
                        }),
                    ],
                },
                Message {
                    annotations: Default::default(),
                    role: Role::Tool,
                    content: vec![Content::ToolResult(ToolResult::success("call_0", "1"))],
                },
            ],
            tools: vec![],
            params: ChatParams {
                model: "mistral-large-latest".into(),
                thinking: ThinkingEffort::High,
                ..ChatParams::new("mistral-large-latest")
            },
        };

        let body = serde_json::to_value(request_body(&req, &MISTRAL)).expect("body serializes");
        let assistant = &body["messages"][0];
        // Mistral's AssistantMessage rejects the field with 422; the same
        // trace rides in `content` as a structured thinking chunk instead.
        assert!(assistant.get("reasoning_content").is_none(), "{body}");
        assert!(assistant.get("reasoning").is_none(), "{body}");
        assert_eq!(
            assistant["content"],
            json!([
                {
                    "type": "thinking",
                    "thinking": [{ "type": "text", "text": "because" }]
                },
                { "type": "text", "text": "checking" }
            ]),
            "{body}"
        );
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "echo");
        // Its stream always reports usage, so the opt-in is not sent.
        assert!(body.get("stream_options").is_none(), "{body}");
        // The effort field is shared and still rides through.
        assert_eq!(body["reasoning_effort"], json!("high"), "{body}");
    }

    #[test]
    fn effort_levels_clamp_to_the_model_s_own_set() {
        use rhema_core::request::ThinkingEffort::*;

        let glm = ["low", "high", "max"].map(String::from);
        // The exact fits and the default fly as they are.
        assert_eq!(clamped_effort(High, &glm), High);
        assert_eq!(clamped_effort(Low, &glm), Low);
        assert_eq!(clamped_effort(Default, &glm), Default);
        // A `medium` the model skips lands on its `high`.
        assert_eq!(clamped_effort(Medium, &glm), High);
        // The level the model refuses moves to the next one it speaks.
        assert_eq!(clamped_effort(High, &["max"].map(String::from)), Default);

        let jump = ["none", "high"].map(String::from);
        // A `low` the model cannot honor is its own default: none.
        assert_eq!(clamped_effort(Low, &jump), Default);
        assert_eq!(clamped_effort(Medium, &jump), High);
        assert_eq!(clamped_effort(High, &jump), High);

        let falls_back = ["low", "medium"].map(String::from);
        assert_eq!(clamped_effort(High, &falls_back), Medium);
        assert_eq!(clamped_effort(Medium, &falls_back), Medium);

        // No catalog knowledge stands as chosen.
        let unknown: Vec<String> = Vec::new();
        assert_eq!(clamped_effort(Medium, &unknown), Medium);
    }

    #[test]
    fn a_base_url_override_rebuilds_the_transport() {
        let provider = MistralProvider::new("k").with_base_url("http://localhost:1234/v1/");
        assert_eq!(provider.base_url, "http://localhost:1234/v1");
    }
}
