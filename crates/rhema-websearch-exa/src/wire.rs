//! The response's serde shape and the pure mapping onto the seam's
//! results. Always compiled: a host without sockets maps its own HTTP
//! through [`results_from_response`].

use rhema_websearch::SearchResult;
use serde::Deserialize;

/// The response Exa returns. `requestId` and the cost fields fall out
/// of the deserialization; only the hits are the seam's business.
#[derive(Debug, Clone, Deserialize)]
pub struct ExaResponse {
    /// The hits, in Exa's ranking order. Absent on a 200: zero
    /// results, an honest answer.
    #[serde(default)]
    pub results: Vec<ExaResult>,
}

/// One hit, as Exa reports it. Every field is `Option`: the mapping
/// drops a hit missing the one it cannot do without (a url to link)
/// rather than fail the whole response over one row.
#[derive(Debug, Clone, Deserialize)]
pub struct ExaResult {
    /// The page's title.
    pub title: Option<String>,
    /// The page's absolute url.
    pub url: Option<String>,
    /// The page text's head, capped by the request's
    /// `contents.text.maxCharacters`.
    pub text: Option<String>,
}

/// Maps a response onto the seam's results, in vendor order, hits
/// missing a url dropped, missing titles filled with the url, text
/// heads carried as the snippet verbatim (the renderer trims).
pub fn results_from_response(response: &ExaResponse) -> Vec<SearchResult> {
    response
        .results
        .iter()
        .filter_map(|result| {
            let url = result.url.as_deref()?.to_owned();
            let title = result.title.clone().unwrap_or_else(|| url.clone());
            let snippet = result.text.clone().unwrap_or_default();
            Some(SearchResult::new(title, url, snippet))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_results_map_in_vendor_order() {
        let response: ExaResponse = serde_json::from_str(
            r#"{
                "requestId": "abc",
                "results": [
                    {"id": "1", "title": "First", "url": "https://a", "text": "one", "score": 0.9},
                    {"id": "2", "title": "Second", "url": "https://b", "publishedDate": "2026-01-01"}
                ]
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[0].snippet, "one");
        assert_eq!(results[1].snippet, "", "no text field, no snippet");
    }

    #[test]
    fn a_missing_results_array_is_zero_results() {
        let response: ExaResponse = serde_json::from_str(r#"{"requestId": "abc"}"#).unwrap();
        assert!(results_from_response(&response).is_empty());
    }

    #[test]
    fn a_hit_without_a_url_drops_and_a_hit_without_a_title_takes_the_url() {
        let response: ExaResponse = serde_json::from_str(
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
