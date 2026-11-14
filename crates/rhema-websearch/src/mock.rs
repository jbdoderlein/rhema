//! The scripted provider: what the chain's tests and a frontend's
//! offline tests drive instead of a vendor. A feature (`mock`), not a
//! dev-dependency, because consumers of this crate need it in their
//! own test graphs; production code never names it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::error::SearchError;
use crate::provider::WebSearchProvider;
use crate::query::SearchQuery;
use crate::result::SearchResult;

/// One provider whose answers come off a script: push outcomes, then
/// search. An empty script answers honestly with no results, so a
/// test that only cares about the first entry of a chain can leave
/// the rest blank.
pub struct MockSearchProvider {
    kind: &'static str,
    script: Mutex<VecDeque<Result<Vec<SearchResult>, SearchError>>>,
    delay: Duration,
    calls: AtomicUsize,
    last_query: Mutex<Option<SearchQuery>>,
}

impl MockSearchProvider {
    /// A provider of `kind` with no script and no delay.
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            script: Mutex::new(VecDeque::new()),
            delay: Duration::ZERO,
            calls: AtomicUsize::new(0),
            last_query: Mutex::new(None),
        }
    }

    /// Appends one successful answer.
    pub fn push_ok(&self, results: Vec<SearchResult>) -> &Self {
        self.script.lock().unwrap().push_back(Ok(results));
        self
    }

    /// Appends one failure.
    pub fn push_err(&self, err: SearchError) -> &Self {
        self.script.lock().unwrap().push_back(Err(err));
        self
    }

    /// Sleeps this long before every answer, the budget tests' stand-in
    /// for a slow vendor.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// How many times `search` ran. The chain's "first success stops
    /// the chain" property is a negative claim about the losers, and
    /// this is how a test sees it.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// A shared handle, the shape the chain's `push` takes.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[async_trait]
impl WebSearchProvider for MockSearchProvider {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_query.lock().unwrap() = Some(query.clone());
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let next = self.script.lock().unwrap().pop_front();
        next.unwrap_or(Ok(Vec::new()))
    }
}

impl MockSearchProvider {
    /// The query the last `search` call carried, the chain's clamping
    /// under test.
    pub fn last_query(&self) -> Option<SearchQuery> {
        self.last_query.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_script_answers_honestly() {
        let provider = MockSearchProvider::new("mock");
        let answer = provider.search(&SearchQuery::new("q")).await;
        assert_eq!(answer, Ok(Vec::new()));
        assert_eq!(provider.calls(), 1);
        assert_eq!(provider.last_query().unwrap().text, "q");
    }
}
