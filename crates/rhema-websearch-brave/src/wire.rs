//! The response's serde shape and the pure mapping onto the seam's
//! results. Always compiled: a host without sockets maps its own HTTP
//! through [`results_from_response`].

use rhema_websearch::SearchResult;
use serde::Deserialize;

/// The response Brave returns for both routes. Only the sections this
/// adapter reads are named; everything else falls out of the
/// deserialization.
#[derive(Debug, Clone, Deserialize)]
pub struct BraveResponse {
    /// The general index's section, present on the web route.
    pub web: Option<BraveSection>,
    /// The news index's section, the shape older news responses used.
    pub news: Option<BraveSection>,
    /// The news route's hits. The current news response puts them at
    /// the top level (`{"type": "news", "results": [...]}`), not
    /// under a `news` key; [`results_from_response`] reads this when
    /// no `news` section is present.
    #[serde(default)]
    pub results: Vec<BraveResult>,
}

/// One index's section: the hits and the rest Brave attaches to them.
#[derive(Debug, Clone, Deserialize)]
pub struct BraveSection {
    /// The hits, in Brave's ranking order.
    pub results: Vec<BraveResult>,
}

/// One hit, as Brave reports it. Every field is `Option`: Brave
/// documents them all, but the mapping drops a hit missing the two it
/// cannot do without (a url to link, and a title to fall back to the
/// url for) rather than fail the whole response over one row.
#[derive(Debug, Clone, Deserialize)]
pub struct BraveResult {
    /// The page's title.
    pub title: Option<String>,
    /// The page's absolute url.
    pub url: Option<String>,
    /// The description, plaintext when the request sent
    /// `text_decorations=false`.
    pub description: Option<String>,
}

/// Maps a response onto the seam's results: the web section, or the
/// news route's top-level `results` (a `news` section is honored when
/// present), in vendor order, hits missing a url dropped, missing
/// titles filled with the url. An absent route is zero results, an
/// honest answer (see the crate docs).
pub fn results_from_response(response: &BraveResponse, news: bool) -> Vec<SearchResult> {
    let results = if news {
        // The documented news shape is top-level; a `news` section is
        // still honored for responses that wrap their hits.
        response
            .news
            .as_ref()
            .map_or(response.results.as_slice(), |section| {
                section.results.as_slice()
            })
    } else {
        response
            .web
            .as_ref()
            .map_or(&[][..], |section| section.results.as_slice())
    };
    results
        .iter()
        .filter_map(|result| {
            let url = result.url.as_deref()?.to_owned();
            let title = result.title.clone().unwrap_or_else(|| url.clone());
            let snippet = result.description.clone().unwrap_or_default();
            Some(SearchResult::new(title, url, snippet))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_section_maps_in_vendor_order() {
        let response: BraveResponse = serde_json::from_str(
            r#"{
                "query": {"original": "q", "safe_search": false},
                "web": {
                    "results": [
                        {"title": "First", "url": "https://a", "description": "one"},
                        {"title": "Second", "url": "https://b", "description": "two"}
                    ]
                },
                "discussions": {"results": []}
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response, false);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[0].url, "https://a");
        assert_eq!(results[0].snippet, "one");
        assert_eq!(results[1].url, "https://b");
    }

    #[test]
    fn the_news_route_reads_its_own_key() {
        let response: BraveResponse = serde_json::from_str(
            r#"{
                "web": {"results": [{"title": "wrong", "url": "https://web", "description": ""}]},
                "news": {"results": [{"title": "right", "url": "https://news", "description": "n"}]}
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response, true);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://news");
    }

    #[test]
    fn the_documented_news_shape_reads_the_top_level_results() {
        // Brave's news route answers `{"type": "news", "results": [...]}`;
        // the hits are not wrapped in a `news` section.
        let response: BraveResponse = serde_json::from_str(
            r#"{
                "type": "news",
                "query": {"original": "q"},
                "results": [
                    {"title": "One", "url": "https://one", "description": "a"},
                    {"title": "Two", "url": "https://two", "description": "b"}
                ]
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response, true);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://one");
        assert_eq!(results[1].title, "Two");
        assert!(
            results_from_response(&response, false).is_empty(),
            "the web route never reads the news hits"
        );
    }

    #[test]
    fn a_missing_section_is_zero_results() {
        let response: BraveResponse =
            serde_json::from_str(r#"{"query": {"original": "q"}}"#).unwrap();
        assert!(results_from_response(&response, false).is_empty());
        assert!(results_from_response(&response, true).is_empty());
    }

    #[test]
    fn a_hit_without_a_url_drops_and_a_hit_without_a_title_takes_the_url() {
        let response: BraveResponse = serde_json::from_str(
            r#"{
                "web": {"results": [
                    {"title": "no url", "description": "d"},
                    {"url": "https://titled", "description": "d"},
                    {"url": "https://untitled", "description": ""}
                ]}
            }"#,
        )
        .unwrap();
        let results = results_from_response(&response, false);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "https://titled");
        assert_eq!(results[1].title, "https://untitled");
    }

    #[test]
    fn a_missing_snippet_is_empty_not_an_error() {
        let response: BraveResponse =
            serde_json::from_str(r#"{"web": {"results": [{"title": "T", "url": "https://a"}]}}"#)
                .unwrap();
        let results = results_from_response(&response, false);
        assert_eq!(results[0].snippet, "");
    }
}
