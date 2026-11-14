//! The provider: Tavily's search route over reqwest, feature
//! `native`.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;

use rhema_websearch::{SearchError, SearchQuery, SearchResult, WebSearchProvider};

use crate::wire::{TavilyResponse, results_from_response};

/// The API root: one search route, the index choice is a body topic.
pub const DEFAULT_BASE_URL: &str = "https://api.tavily.com";
/// The search route.
pub const SEARCH_PATH: &str = "/search";
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// Tavily search behind the [`WebSearchProvider`] seam. One client
/// per provider, built in the constructor: reqwest pools
/// connections, and the chain holds providers for the process's
/// life. No request timeout here, the chain's budgets own the clock
/// (`specs/rhema-websearch.md` §5); the client keeps only the connect
/// timeout, so a dead network fails fast instead of hanging the
/// attempt's slot.
pub struct TavilyProvider {
    api_key: String,
    base_url: String,
    http: Client,
}

impl TavilyProvider {
    /// A provider authenticating `api_key` against
    /// [`DEFAULT_BASE_URL`], bearer auth.
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
impl WebSearchProvider for TavilyProvider {
    fn kind(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        // One route: the news flag is a topic, not a path. The depth
        // and the two include flags are the crate docs' shapes: basic
        // (one credit), no vendor answer, no raw pages.
        let body = json!({
            "query": query.text,
            "topic": if query.news { "news" } else { "general" },
            "search_depth": "basic",
            "max_results": query.max_results,
            "include_answer": false,
            "include_raw_content": false,
        });
        let response = self
            .http
            .post(format!("{}{SEARCH_PATH}", self.base_url))
            .bearer_auth(&self.api_key)
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
        let body: TavilyResponse = response
            .json()
            .await
            .map_err(|err| SearchError::Parse(err.to_string()))?;
        Ok(results_from_response(&body))
    }
}
