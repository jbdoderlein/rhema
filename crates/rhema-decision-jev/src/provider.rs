//! The provider: Jev's one route over reqwest, feature `native`.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

use rhema_decision::{DecisionError, DecisionProvider, DecisionRequest, DecisionResponse};

use crate::wire::{DEFAULT_MODEL, request_body, response_from_json};

/// The API root: the System One route hangs off it.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// The one route.
pub const SYSTEMONE_PATH: &str = "/v1/systemone";
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// Jev behind the [`DecisionProvider`] seam. One client per provider,
/// built in the constructor: reqwest pools connections, and the chain
/// holds providers for the process's life. No request timeout here,
/// the chain's budgets own the clock (`specs/rhema-decision.md` §5);
/// the client keeps only the connect timeout, so a dead network fails
/// fast instead of hanging the attempt's slot.
pub struct JevProvider {
    api_key: String,
    base_url: String,
    model: String,
    http: Client,
}

impl JevProvider {
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

    /// Asks for another model: the alias `jev-preview`, or a pinned
    /// version for runs that compare.
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
impl DecisionProvider for JevProvider {
    fn kind(&self) -> &'static str {
        "jev"
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let response = self
            .http
            .post(format!("{}{SYSTEMONE_PATH}", self.base_url))
            .bearer_auth(&self.api_key)
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
