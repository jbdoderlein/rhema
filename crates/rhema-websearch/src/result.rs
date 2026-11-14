//! One hit: a title, a url, a snippet.

/// One search result, the shape every adapter normalizes to. The
/// vendor's ranking is the product, so results arrive in vendor order
/// and nothing here re-sorts them; there is no score field for the
/// same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The page's title, as the vendor reports it.
    pub title: String,
    /// The page's absolute url.
    pub url: String,
    /// The vendor's snippet: Brave's description, Exa's page-text
    /// head, Tavily's content chunk. Plaintext by contract: the
    /// adapters strip vendor markup before they return (Brave's
    /// decorations are switched off at request time).
    pub snippet: String,
}

impl SearchResult {
    /// One hit from its three fields.
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
        }
    }
}
