//! The provider: Clef's model routes on Workers AI over reqwest,
//! feature `native`.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

use rhema_decision::{DecisionError, DecisionProvider, DecisionRequest, DecisionResponse};

use crate::wire::{ClefModel, request_body, route_path, unwrap_envelope};

/// The API root: the account-scoped model routes hang off it.
pub const DEFAULT_BASE_URL: &str = "https://api.cloudflare.com/client/v4";
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// Clef behind the [`DecisionProvider`] seam. One client per provider,
/// built in the constructor: reqwest pools connections, and the chain
/// holds providers for the process's life. No request timeout here,
/// the chain's budgets own the clock (`specs/rhema-decision.md` §5);
/// the client keeps only the connect timeout, so a dead network fails
/// fast instead of hanging the attempt's slot.
pub struct ClefProvider {
    account_id: String,
    auth_token: String,
    base_url: String,
    model: ClefModel,
    http: Client,
}

impl ClefProvider {
    /// A provider running [`ClefModel::Clef`] on `account_id`,
    /// authenticating `auth_token` (a Workers AI API token) against
    /// [`DEFAULT_BASE_URL`].
    pub fn new(account_id: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            auth_token: auth_token.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            model: ClefModel::default(),
            http: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(USER_AGENT)
                .build()
                .expect("a client builder with only timeouts cannot fail"),
        }
    }

    /// Runs the other Clef: the model is the route on Workers AI, so
    /// this picks `clef-flash`'s.
    pub fn with_model(mut self, model: ClefModel) -> Self {
        self.model = model;
        self
    }

    /// Overrides the base URL, for tests against a mock server. The
    /// spelling is the API root (no `/accounts/...`), trailing slashes
    /// trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }
}

#[async_trait]
impl DecisionProvider for ClefProvider {
    fn kind(&self) -> &'static str {
        "clef"
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let response = self
            .http
            .post(format!(
                "{}{}",
                self.base_url,
                route_path(&self.account_id, self.model)
            ))
            .bearer_auth(&self.auth_token)
            .json(&request_body(request, self.model))
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
        // Read once: the body serves both arms, success and failure
        // ride the same envelope.
        let body = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(DecisionError::from_status(status, &body));
        }
        unwrap_envelope(&body, status)
    }
}
