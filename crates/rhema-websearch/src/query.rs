//! The search request: the text, how many hits, which index.

/// The result count a query asks for when the caller named none.
pub const DEFAULT_MAX_RESULTS: u8 = 8;
/// The result-count floor. A vendor param of zero is an error
/// everywhere, so the clamp removes the case instead of translating
/// it.
pub const MIN_MAX_RESULTS: u8 = 1;
/// The result-count ceiling: Brave's `count` param tops out here, and
/// the other two vendors accept it, so one number serves all three
/// adapters.
pub const MAX_MAX_RESULTS: u8 = 20;

/// One search request, the whole vocabulary an adapter may act on.
/// Vendor knobs beyond these (safe search, regions, date filters,
/// depth) stay out until a use names one; see
/// `specs/rhema-websearch.md` §0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// The query text, verbatim, no rewriting on the way down.
    pub text: String,
    /// How many hits to ask for, already clamped to
    /// [`MIN_MAX_RESULTS`]..=[`MAX_MAX_RESULTS`] by
    /// [`SearchQuery::new`].
    pub max_results: u8,
    /// The news index instead of the general one. Brave maps this to
    /// its news endpoint, Exa to its news category, Tavily to its
    /// news topic (see `specs/rhema-websearch.md` §4).
    pub news: bool,
}

impl SearchQuery {
    /// A general-index query with [`DEFAULT_MAX_RESULTS`] results.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            max_results: DEFAULT_MAX_RESULTS,
            news: false,
        }
    }

    /// Sets the result count, clamped to the vendors' shared range.
    pub fn with_max_results(mut self, max_results: u8) -> Self {
        self.max_results = max_results.clamp(MIN_MAX_RESULTS, MAX_MAX_RESULTS);
        self
    }

    /// Asks the news index.
    pub fn with_news(mut self, news: bool) -> Self {
        self.news = news;
        self
    }

    /// The query as the chain runs it: the count clamped again, so an
    /// adapter can never see an out-of-range value even if the struct
    /// was built by hand with a bad field.
    pub fn clamped(&self) -> Self {
        let mut clamped = self.clone();
        clamped.max_results = self.max_results.clamp(MIN_MAX_RESULTS, MAX_MAX_RESULTS);
        clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_shared_shape() {
        let query = SearchQuery::new("rust async book");
        assert_eq!(query.text, "rust async book");
        assert_eq!(query.max_results, DEFAULT_MAX_RESULTS);
        assert!(!query.news);
    }

    #[test]
    fn the_count_clamps_into_the_vendors_shared_range() {
        assert_eq!(SearchQuery::new("q").with_max_results(0).max_results, 1);
        assert_eq!(
            SearchQuery::new("q").with_max_results(100).max_results,
            MAX_MAX_RESULTS
        );
        assert_eq!(SearchQuery::new("q").with_max_results(5).max_results, 5);
    }

    #[test]
    fn clamped_tames_a_hand_built_struct() {
        let mut query = SearchQuery::new("q");
        query.max_results = 0;
        assert_eq!(query.clamped().max_results, MIN_MAX_RESULTS);
        query.max_results = 255;
        assert_eq!(query.clamped().max_results, MAX_MAX_RESULTS);
    }
}
