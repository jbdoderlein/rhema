//! The provider: Brave's two routes over reqwest, feature `native`.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

use rhema_websearch::{SearchError, SearchQuery, SearchResult, WebSearchProvider};

use crate::wire::{BraveResponse, results_from_response};

/// The API root: the web and news routes hang off it.
pub const DEFAULT_BASE_URL: &str = "https://api.search.brave.com/res";
/// The general index's route.
pub const BRAVE_WEB_PATH: &str = "/web/search";
/// The news index's route.
pub const BRAVE_NEWS_PATH: &str = "/news/search";
/// The user agent the provider sends, the house spelling.
pub const USER_AGENT: &str = concat!("rhema/", env!("CARGO_PKG_VERSION"));

/// Brave Search behind the [`WebSearchProvider`] seam. One client per
/// provider, built in the constructor: reqwest pools connections, and
/// the chain holds providers for the process's life. No request
/// timeout here, the chain's budgets own the clock
/// (`specs/rhema-websearch.md` §5); the client keeps only the connect
/// timeout, so a dead network fails fast instead of hanging the
/// attempt's slot.
pub struct BraveProvider {
    api_key: String,
    base_url: String,
    http: Client,
}

impl BraveProvider {
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

    /// Overrides the base URL, for tests against a mock server. The
    /// spelling is the API root (no route), trailing slashes trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Runs one route and maps the response. The count and the news
    /// flag are the query's; `text_decorations=false` is the
    /// plaintext contract (see the crate docs).
    async fn run_route(
        &self,
        route: &str,
        query: &SearchQuery,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let response = self
            .http
            .get(format!("{}{route}", self.base_url))
            .header("X-Subscription-Token", &self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[
                ("q", query.text.as_str()),
                ("count", &query.max_results.to_string()),
                ("text_decorations", "false"),
            ])
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
        let body: BraveResponse = response
            .json()
            .await
            .map_err(|err| SearchError::Parse(err.to_string()))?;
        Ok(results_from_response(&body, query.news))
    }
}

#[async_trait]
impl WebSearchProvider for BraveProvider {
    fn kind(&self) -> &'static str {
        "brave"
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        let route = if query.news {
            BRAVE_NEWS_PATH
        } else {
            BRAVE_WEB_PATH
        };
        self.run_route(route, query).await
    }
}
