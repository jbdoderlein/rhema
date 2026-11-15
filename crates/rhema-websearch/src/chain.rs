//! The fallback chain: configured providers in the user's order, one
//! query, first success wins.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::time::timeout;

use crate::error::{ChainError, SearchError};
use crate::provider::WebSearchProvider;
use crate::query::SearchQuery;
use crate::result::SearchResult;

/// One attempt's budget. A vendor that cannot answer in this either
/// answers too slowly to read or is down; the value sits well under
/// the shell tool's command timeout, so the chain's own error text
/// reaches the model instead of a killed command's.
pub const DEFAULT_PER_ATTEMPT: Duration = Duration::from_secs(15);

/// The whole chain's budget. Four vendors at the per-attempt cap
/// would still overrun a turn's patience, and the budget, not the
/// count, is what bounds the ladder: the chain stops before an
/// attempt that cannot fit.
pub const DEFAULT_BUDGET: Duration = Duration::from_secs(45);

/// One configured provider: the instance name the config and the
/// output use, the adapter behind it.
#[derive(Clone)]
struct Entry {
    name: String,
    provider: Arc<dyn WebSearchProvider>,
}

/// What a successful chain run answers: who answered, and the hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOutcome {
    /// The instance name of the entry that answered (`brave-main`),
    /// the config's spelling.
    pub name: String,
    /// That entry's vendor kind (`brave`).
    pub kind: &'static str,
    /// The hits, in vendor order. Possibly empty: an empty result set
    /// is a success (see below).
    pub results: Vec<SearchResult>,
}

/// The ordered fallback: attempts run sequentially, every error falls
/// through, the first success stops the chain, an empty result set
/// included. Racing would bill every provider for one query, and the
/// ladder exists to survive failures, not to shave milliseconds (see
/// `specs/rhema-websearch.md` §5, §12.6).
#[derive(Clone)]
pub struct Chain {
    entries: Vec<Entry>,
    per_attempt: Duration,
    budget: Duration,
}

impl Chain {
    /// The default budgets: [`DEFAULT_PER_ATTEMPT`] and
    /// [`DEFAULT_BUDGET`].
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            per_attempt: DEFAULT_PER_ATTEMPT,
            budget: DEFAULT_BUDGET,
        }
    }

    /// Appends one configured provider. `name` is the instance name
    /// the config and the output use, not the vendor kind, so error
    /// text reads like the settings do.
    pub fn push(&mut self, name: impl Into<String>, provider: Arc<dyn WebSearchProvider>) {
        self.entries.push(Entry {
            name: name.into(),
            provider,
        });
    }

    /// How many providers the chain holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The chain's instance names, in order: what a frontend's UI
    /// shows for "search runs through", and what tests read to see
    /// the order without running it.
    pub fn names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    /// Whether the chain holds no provider: `search` on it fails
    /// before any attempt, with the aggregate saying so.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Overrides the per-attempt budget. Tests only, in practice; the
    /// default is the product's number.
    #[cfg(test)]
    fn with_per_attempt(mut self, per_attempt: Duration) -> Self {
        self.per_attempt = per_attempt;
        self
    }

    /// Overrides the whole-chain budget. Tests only, in practice.
    #[cfg(test)]
    fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// Runs the query down the chain.
    ///
    /// The count is clamped at the entry (`specs/rhema-websearch.md`
    /// §0: the chain is where the vendors' shared range is enforced),
    /// the budget bounds the ladder, and the attempts that ran and
    /// failed land in the [`ChainError`] in order, so a total failure
    /// names every reason.
    pub async fn search(&self, query: &SearchQuery) -> Result<ChainOutcome, ChainError> {
        let query = query.clamped();
        let started = Instant::now();
        let deadline = started + self.budget;
        let mut attempts = Vec::new();
        for entry in &self.entries {
            // The chain stops before an attempt that cannot fit its
            // budget; what ran already is the aggregate's story.
            let remaining = deadline.checked_duration_since(Instant::now());
            let Some(remaining) = remaining else {
                break;
            };
            let attempt_budget = self.per_attempt.min(remaining);
            let provider = entry.provider.clone();
            let outcome = timeout(attempt_budget, provider.search(&query)).await;
            match outcome {
                Ok(Ok(results)) => {
                    return Ok(ChainOutcome {
                        name: entry.name.clone(),
                        kind: entry.provider.kind(),
                        results,
                    });
                }
                Ok(Err(err)) => attempts.push((entry.name.clone(), err)),
                // The attempt's own budget, not the vendor's answer.
                Err(_elapsed) => attempts.push((entry.name.clone(), SearchError::Timeout)),
            }
        }
        Err(ChainError::new(attempts))
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::mock::MockSearchProvider;

    fn hit(n: &str) -> SearchResult {
        SearchResult::new(n, format!("https://x/{n}"), "snippet")
    }

    #[tokio::test]
    async fn first_success_stops_the_chain_and_names_the_winner() {
        let loser = MockSearchProvider::new("exa").shared();
        loser.push_err(SearchError::Auth);
        let winner = MockSearchProvider::new("brave").shared();
        winner.push_ok(vec![hit("a")]);
        let mut chain = Chain::new();
        chain.push("exa-work", loser.clone());
        chain.push("brave-main", winner.clone());

        let outcome = chain.search(&SearchQuery::new("q")).await.unwrap();
        assert_eq!(outcome.name, "brave-main");
        assert_eq!(outcome.kind, "brave");
        assert_eq!(outcome.results, vec![hit("a")]);
        // The chain reached the winner only.
        assert_eq!(winner.calls(), 1);
    }

    #[tokio::test]
    async fn an_empty_result_set_is_a_success_not_a_fall_through() {
        let first = MockSearchProvider::new("brave").shared();
        first.push_ok(Vec::new());
        let second = MockSearchProvider::new("exa").shared();
        let mut chain = Chain::new();
        chain.push("brave-main", first.clone());
        chain.push("exa-work", second.clone());

        let outcome = chain.search(&SearchQuery::new("q")).await.unwrap();
        assert_eq!(outcome.name, "brave-main");
        assert!(outcome.results.is_empty());
        assert_eq!(second.calls(), 0, "an honest answer bills no second vendor");
    }

    #[tokio::test]
    async fn every_error_falls_through_and_the_aggregate_orders_them() {
        let brave = MockSearchProvider::new("brave").shared();
        brave.push_err(SearchError::RateLimited);
        let exa = MockSearchProvider::new("exa").shared();
        exa.push_err(SearchError::Auth);
        let tavily = MockSearchProvider::new("tavily").shared();
        tavily.push_ok(vec![hit("late")]);
        let mut chain = Chain::new();
        chain.push("brave-main", brave);
        chain.push("exa-work", exa);
        chain.push("tavily-main", tavily);

        let outcome = chain.search(&SearchQuery::new("q")).await.unwrap();
        assert_eq!(outcome.name, "tavily-main");
    }

    #[tokio::test]
    async fn a_total_failure_carries_every_attempt_in_order() {
        let brave = MockSearchProvider::new("brave").shared();
        brave.push_err(SearchError::RateLimited);
        let exa = MockSearchProvider::new("exa").shared();
        exa.push_err(SearchError::Auth);
        let mut chain = Chain::new();
        chain.push("brave-main", brave);
        chain.push("exa-work", exa);

        let err = chain.search(&SearchQuery::new("q")).await.unwrap_err();
        assert_eq!(
            err.attempts,
            vec![
                ("brave-main".to_owned(), SearchError::RateLimited),
                ("exa-work".to_owned(), SearchError::Auth),
            ]
        );
        assert_eq!(
            err.to_string(),
            "every provider failed: brave-main: rate limited\nexa-work: the key or plan was refused"
        );
    }

    #[tokio::test]
    async fn a_slow_attempt_times_out_and_the_chain_continues() {
        let slow = MockSearchProvider::new("brave")
            .with_delay(Duration::from_millis(500))
            .shared();
        slow.push_err(SearchError::Transport("never answered".into()));
        let fast = MockSearchProvider::new("exa").shared();
        fast.push_ok(vec![hit("fast")]);
        let mut chain = Chain::new().with_per_attempt(Duration::from_millis(50));
        chain.push("brave-main", slow);
        chain.push("exa-work", fast);

        let outcome = chain.search(&SearchQuery::new("q")).await.unwrap();
        assert_eq!(outcome.name, "exa-work");
    }

    #[tokio::test]
    async fn the_chain_budget_stops_the_ladder_before_the_next_attempt() {
        let brave = MockSearchProvider::new("brave")
            .with_delay(Duration::from_millis(150))
            .shared();
        brave.push_err(SearchError::RateLimited);
        let exa = MockSearchProvider::new("exa").shared();
        let mut chain = Chain::new()
            .with_per_attempt(Duration::from_secs(10))
            .with_budget(Duration::from_millis(100));
        chain.push("brave-main", brave.clone());
        chain.push("exa-work", exa.clone());

        let err = chain.search(&SearchQuery::new("q")).await.unwrap_err();
        assert_eq!(err.attempts.len(), 1, "only the attempt that fit ran");
        assert_eq!(exa.calls(), 0);
    }

    #[tokio::test]
    async fn an_empty_chain_fails_without_running_anything() {
        let err = Chain::new()
            .search(&SearchQuery::new("q"))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "every provider failed: no provider was configured"
        );
    }

    #[tokio::test]
    async fn the_count_is_clamped_at_the_entry() {
        let provider = MockSearchProvider::new("brave").shared();
        let mut chain = Chain::new();
        chain.push("brave-main", provider.clone());
        let mut query = SearchQuery::new("q");
        query.max_results = 0;
        chain.search(&query).await.unwrap();
        assert_eq!(
            provider.last_query().unwrap().max_results,
            crate::query::MIN_MAX_RESULTS
        );
    }

    #[tokio::test]
    async fn len_and_is_empty_report_the_entries() {
        let mut chain = Chain::new();
        assert!(chain.is_empty());
        chain.push("brave-main", MockSearchProvider::new("brave").shared());
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
    }
}
