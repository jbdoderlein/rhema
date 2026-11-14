//! The response's serde shape and the pure mapping onto the seam's
//! results. Always compiled: a host without sockets maps its own HTTP
//! through [`results_from_response`].

use rhema_websearch::SearchResult;
use serde::Deserialize;

/// The response Tavily returns. `query`, `answer`, `responseTime` and
/// the rest fall out of the deserialization; only the hits are the
/// seam's business.
#[derive(Debug, Clone, Deserialize)]
pub struct TavilyResponse {
    /// The hits, in Tavily's ranking order. Absent on a 200: zero
    /// results, an honest answer.
    #[serde(default)]
    pub results: Vec<TavilyResult>,
}

/// One hit, as Tavily reports it. Every field is `Option`: the
/// mapping drops a hit missing the one it cannot do without (a url to
/// link) rather than fail the whole response over one row.
#[derive(Debug, Clone, Deserialize)]
pub struct TavilyResult {
    /// The page's title.
    pub title: Option<String>,
    /// The page's absolute url.
    pub url: Option<String>,
    /// The relevance-picked chunk, the snippet Tavily hands back.
    pub content: Option<String>,
}

/// Maps a response onto the seam's results, in vendor order, hits
/// missing a url dropped, missing titles filled with the url,
/// content chunks carried as the snippet verbatim (the renderer
/// trims).
pub fn results_from_response(response: &TavilyResponse) -> Vec<SearchResult> {
    response
        .results
        .iter()
        .filter_map(|result| {
            let url = result.url.as_deref()?.to_owned();
            let title = result.title.clone().unwrap_or_else(|| url.clone());
            let snippet = result.content.clone().unwrap_or_default();
            Some(SearchResult::new(title, url, snippet))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_results_map_in_vendor_order() {
        let response: TavilyResponse = serde_json::from_str(
            r#"{
                "query": "q",
                "answer": "ignored",
                "responseTime": 1.2,
                "results": [
                    {"title": "First", "url": "https://a", "content": "one", "score": 0.9,
                     "raw_content": "ignored"},
                    {"title": "Second", "url": "https://b", "score": 0.8}
                ]
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].snippet, "one");
        assert_eq!(results[1].snippet, "", "no content field, no snippet");
    }

    #[test]
    fn a_missing_results_array_is_zero_results() {
        let response: TavilyResponse = serde_json::from_str(r#"{"query": "q"}"#).unwrap();
        assert!(results_from_response(&response).is_empty());
    }

    #[test]
    fn a_hit_without_a_url_drops_and_a_hit_without_a_title_takes_the_url() {
        let response: TavilyResponse = serde_json::from_str(
            r#"{
                "results": [
                    {"title": "no url"},
                    {"url": "https://untitled"}
                ]
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "https://untitled");
    }
}
