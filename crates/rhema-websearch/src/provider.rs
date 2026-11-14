//! The adapter seam: one vendor's search API behind one method.

use async_trait::async_trait;

use crate::error::SearchError;
use crate::query::SearchQuery;
use crate::result::SearchResult;

/// One vendor's search. The chain holds these behind the trait; a
/// frontend holds a whole chain behind `Arc<Chain>`. Send + Sync like
/// rhema-core's other seams, attempts are short awaited HTTP calls.
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    /// The vendor spelling the config uses: `brave`, `exa`,
    /// `tavily`. A constant per adapter, it names the error and log
    /// lines, never a second constructor path.
    fn kind(&self) -> &'static str;

    /// Runs the query. Results arrive in vendor order, the vendor's
    /// ranking is the product. An empty vector is an honest answer,
    /// not a failure; every failure is one of
    /// [`SearchError`]'s classes.
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, SearchError>;
}
