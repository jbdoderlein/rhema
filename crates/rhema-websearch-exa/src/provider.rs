//! The provider: Exa's search route over reqwest, feature `native`.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;

use rhema_websearch::{SearchError, SearchQuery, SearchResult, WebSearchProvider};

use crate::wire::{ExaResponse, results_from_response};

/// The API root: one search route, the index choice is a body field.
pub const DEFAULT_BASE_URL: &str = "https://api.exa.ai";
/// The search route.
pub const SEARCH_PATH: &str = "/search";
/// The page-text head a hit may carry, the snippet Exa hands back.
/// Uncapped, a text is a whole page per hit; the renderer trims
/// further (to 300 chars), this cap is the wire's own decency.
pub const TEXT_MAX_CHARACTERS: usize = 500;
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// Exa search behind the [`WebSearchProvider`] seam. One client per
/// provider, built in the constructor: reqwest pools connections, and
/// the chain holds providers for the process's life. No request
/// timeout here, the chain's budgets own the clock
/// (`specs/rhema-websearch.md` §5); the client keeps only the connect
/// timeout, so a dead network fails fast instead of hanging the
/// attempt's slot.
pub struct ExaProvider {
    api_key: String,
    base_url: String,
    http: Client,
}

impl ExaProvider {
    /// A provider authenticating `api_key` against
    /// [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            http: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(USER_AGENT)
                .build()
                .expect("a client builder with only timeouts cannot fail"),
        }
    }

    /// Overrides the base URL, for tests against a mock server.
    /// Trailing slashes trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }
}

#[async_trait]
impl WebSearchProvider for ExaProvider {
    fn kind(&self) -> &'static str {
        "exa"
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        // One route: the news flag is a category, not a path.
        let mut body = json!({
            "query": query.text,
            "numResults": query.max_results,
            "contents": {"text": {"maxCharacters": TEXT_MAX_CHARACTERS}},
        });
        if query.news {
            body["category"] = json!("news");
        }
        let response = self
            .http
            .post(format!("{}{SEARCH_PATH}", self.base_url))
            .header("x-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                if err.is_timeout() {
                    SearchError::Timeout
                } else {
                    SearchError::Transport(err.to_string())
                }
            })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let head = response.text().await.unwrap_or_default();
            return Err(SearchError::from_status(status, &head));
        }
        let body: ExaResponse = response
            .json()
            .await
            .map_err(|err| SearchError::Parse(err.to_string()))?;
        Ok(results_from_response(&body))
    }
}
