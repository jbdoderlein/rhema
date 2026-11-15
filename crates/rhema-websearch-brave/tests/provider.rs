//! Offline integration tests: the provider against an in-process
//! wiremock server. Zero network.

use rhema_websearch::{SearchError, SearchQuery, WebSearchProvider};
use rhema_websearch_brave::{BRAVE_NEWS_PATH, BRAVE_WEB_PATH, BraveProvider, DEFAULT_BASE_URL};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn web_body() -> serde_json::Value {
    json!({
        "query": {"original": "rust async", "safe_search": false},
        "web": {
            "results": [
                {"title": "Async Book", "url": "https://rust-lang.github.io/async-book/",
                 "description": "Getting started", "age": "2 days ago"},
                {"title": "Tokio", "url": "https://tokio.rs", "description": "A runtime"}
            ],
            "family_friendly": true
        }
    })
}

#[test]
fn the_default_routes_are_the_documented_endpoints() {
    // The API root is `/res`; the version prefix belongs to the route.
    // A regression here makes every live Brave call 404 while the
    // mock-server tests, which override the base URL, stay green.
    assert_eq!(DEFAULT_BASE_URL, "https://api.search.brave.com/res");
    assert_eq!(
        format!("{DEFAULT_BASE_URL}{BRAVE_WEB_PATH}"),
        "https://api.search.brave.com/res/v1/web/search"
    );
    assert_eq!(
        format!("{DEFAULT_BASE_URL}{BRAVE_NEWS_PATH}"),
        "https://api.search.brave.com/res/v1/news/search"
    );
}

#[tokio::test]
async fn the_web_route_gets_the_key_the_params_and_maps_the_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/web/search"))
        .and(query_param("q", "rust async"))
        .and(query_param("count", "5"))
        // The plaintext contract: undecorated descriptions come back.
        .and(query_param("text_decorations", "false"))
        .and(header("X-Subscription-Token", "test-key"))
        .and(header("Accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(web_body()))
        .mount(&server)
        .await;

    let provider = BraveProvider::new("test-key").with_base_url(server.uri());
    let results = provider
        .search(&SearchQuery::new("rust async").with_max_results(5))
        .await
        .expect("the call succeeds");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Async Book");
    assert_eq!(results[0].url, "https://rust-lang.github.io/async-book/");
    assert_eq!(results[0].snippet, "Getting started");
}

#[tokio::test]
async fn the_news_flag_switches_the_route_and_the_section() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/news/search"))
        .and(query_param("q", "rust release"))
        .respond_with(
            // The documented news shape: the hits sit at the top level.
            ResponseTemplate::new(200).set_body_json(json!({
                "type": "news",
                "results": [
                    {"title": "Rust 1.90", "url": "https://blog.rust-lang.org", "description": "released"}
                ]
            })),
        )
        .mount(&server)
        .await;
    // The web route must not answer a news query: if it did, the mock
    // would not match and the call would 404.
    Mock::given(method("GET"))
        .and(path("/v1/web/search"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let provider = BraveProvider::new("test-key").with_base_url(server.uri());
    let results = provider
        .search(&SearchQuery::new("rust release").with_news(true))
        .await
        .expect("the call succeeds");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://blog.rust-lang.org");
}

#[tokio::test]
async fn statuses_map_to_their_classes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;
    let provider = BraveProvider::new("bad-key").with_base_url(server.uri());
    assert_eq!(
        provider.search(&SearchQuery::new("q")).await,
        Err(SearchError::Auth)
    );
}

#[tokio::test]
async fn rate_limits_map_to_the_rate_limited_class() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let provider = BraveProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.search(&SearchQuery::new("q")).await,
        Err(SearchError::RateLimited)
    );
}

#[tokio::test]
async fn other_statuses_carry_the_body_head() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;
    let provider = BraveProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.search(&SearchQuery::new("q")).await,
        Err(SearchError::Http {
            status: 500,
            body: "upstream exploded".into()
        })
    );
}

#[tokio::test]
async fn a_malformed_body_is_a_parse_error_the_chain_falls_through_on() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;
    let provider = BraveProvider::new("key").with_base_url(server.uri());
    let err = provider
        .search(&SearchQuery::new("q"))
        .await
        .expect_err("html is not the documented shape");
    assert!(matches!(err, SearchError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_200_without_the_section_is_an_honest_empty_answer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"query": {"original": "q"}, "type": "search"})),
        )
        .mount(&server)
        .await;
    let provider = BraveProvider::new("key").with_base_url(server.uri());
    let results = provider.search(&SearchQuery::new("q")).await.unwrap();
    assert!(
        results.is_empty(),
        "not a Parse error: the chain would bill a second vendor"
    );
}

#[tokio::test]
async fn a_transport_failure_is_the_transport_class() {
    // A port nothing listens on: connect refused, reqwest's own error.
    let provider = BraveProvider::new("key").with_base_url("http://127.0.0.1:9");
    let err = provider
        .search(&SearchQuery::new("q"))
        .await
        .expect_err("nothing is listening");
    assert!(matches!(err, SearchError::Transport(_)), "{err:?}");
}

#[test]
fn the_kind_names_the_config_spelling() {
    assert_eq!(BraveProvider::new("k").kind(), "brave");
}
