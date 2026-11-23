//! The provider: the Decisions route over reqwest, feature `native`.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

use rhema_decision::{DecisionError, DecisionProvider, DecisionRequest, DecisionResponse};

use crate::wire::{DEFAULT_MODEL, request_body, response_from_json};

/// The API root: the Decisions route hangs off it.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai";
/// The one route, alpha: the router's own docs say the shape may
/// move, which is one adapter's `Parse` to break and nothing else.
pub const DECISIONS_PATH: &str = "/api/alpha/decisions";
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));
/// The `HTTP-Referer` attribution header OpenRouter recommends for
/// its rankings, the turn provider's spelling.
pub const APP_REFERER: &str = "https://rhema.rs";
/// The `X-Title` attribution header OpenRouter recommends, the turn
/// provider's spelling.
pub const APP_TITLE: &str = "rhema";

/// The router behind the [`DecisionProvider`] seam. One client per
/// provider, built in the constructor: reqwest pools connections, and
/// the profile holds providers for the process's life. No request
/// timeout here, the call's budgets own the clock
/// (`specs/rhema-decision.md` §5); the client keeps only the connect
/// timeout, so a dead network fails fast instead of hanging the
/// attempt's slot.
pub struct OpenRouterProvider {
    api_key: String,
    base_url: String,
    model: String,
    http: Client,
}

impl OpenRouterProvider {
    /// A provider authenticating `api_key` against
    /// [`DEFAULT_BASE_URL`], asking for [`DEFAULT_MODEL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            http: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(USER_AGENT)
                .build()
                .expect("a client builder with only timeouts cannot fail"),
        }
    }

    /// Asks for another model: any slug the catalog lists under
    /// `output_modalities=decisions`, the `~typesafe/jev-latest`
    /// alias, a `:free` one.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Overrides the base URL, for tests against a mock server. The
    /// spelling is the API root (no route), trailing slashes trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }
}

#[async_trait]
impl DecisionProvider for OpenRouterProvider {
    fn kind(&self) -> &'static str {
        "openrouter"
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let response = self
            .http
            .post(format!("{}{DECISIONS_PATH}", self.base_url))
            .bearer_auth(&self.api_key)
            .header("HTTP-Referer", APP_REFERER)
            .header("X-Title", APP_TITLE)
            .json(&request_body(request, &self.model))
            .send()
            .await
            .map_err(|err| {
                if err.is_timeout() {
                    DecisionError::Timeout
                } else {
                    DecisionError::Transport(err.to_string())
                }
            })?;
        let status = response.status().as_u16();
        // Read once: the body serves the error arm and the parse arm.
        let body = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(DecisionError::from_status(status, &body));
        }
        response_from_json(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhema_decision::{Answer, Question};
    use serde_json::json;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn request() -> DecisionRequest {
        DecisionRequest::new("s").with_question("q", Question::noul("x?"))
    }

    #[tokio::test]
    async fn the_call_rides_the_decisions_route_with_the_key_and_the_attribution() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/alpha/decisions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "gen-dec-1", "model": "typesafe/jev-1.14", "provider": "TypeSafe",
                "answers": {"q": {"type": "noul", "noul": 0.9, "confidence": 0.9}},
                "usage": {"input_tokens": 5, "output_tokens": 0, "cost": 0.00001}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = OpenRouterProvider::new("or-key")
            .with_base_url(server.uri())
            .with_model("~typesafe/jev-latest");
        let response = provider.decide(&request()).await.unwrap();
        assert_eq!(response.model, "typesafe/jev-1.14");
        assert_eq!(response.usage.cost, Some(0.00001));
        let received: Request = server.received_requests().await.unwrap().remove(0);
        assert_eq!(
            received.headers.get("authorization").unwrap(),
            "Bearer or-key"
        );
        assert_eq!(received.headers.get("x-title").unwrap(), "rhema");
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "~typesafe/jev-latest");
        assert_eq!(body["questions"]["q"]["type"], "noul");
    }

    #[tokio::test]
    async fn the_routers_error_envelope_is_invalid_with_its_message() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/api/alpha/decisions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {"code": 400, "message": "No endpoints found for model \"x\""}
            })))
            .mount(&server)
            .await;
        let provider = OpenRouterProvider::new("or-key").with_base_url(server.uri());
        let err = provider.decide(&request()).await.unwrap_err();
        assert_eq!(
            err,
            DecisionError::Invalid("No endpoints found for model \"x\"".into())
        );
        // Out of credits: 402, a `Http` with the same message.
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/api/alpha/decisions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "error": {"code": 402, "message": "Insufficient credits"}
            })))
            .mount(&server)
            .await;
        let provider = OpenRouterProvider::new("or-key").with_base_url(server.uri());
        let err = provider.decide(&request()).await.unwrap_err();
        assert_eq!(err.to_string(), "HTTP 402: Insufficient credits");
    }

    #[tokio::test]
    async fn a_refusal_in_the_body_is_an_answer_not_a_failure() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/api/alpha/decisions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "cloudflare/clef", "answers": {"q": {"type": "refusal"}},
                "usage": {"input_tokens": 1, "output_tokens": 0}
            })))
            .mount(&server)
            .await;
        let provider = OpenRouterProvider::new("or-key").with_base_url(server.uri());
        let response = provider.decide(&request()).await.unwrap();
        assert_eq!(response.answers["q"], Answer::Refusal);
    }

    #[test]
    fn the_kind_is_the_routers() {
        assert_eq!(OpenRouterProvider::new("k").kind(), "openrouter");
    }
}
