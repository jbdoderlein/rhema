//! Offline integration tests: the provider against an in-process
//! wiremock server. Zero network.

use rhema_websearch::{SearchError, SearchQuery, WebSearchProvider};
use rhema_websearch_tavily::TavilyProvider;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn the_search_route_posts_the_bearer_key_the_count_and_the_decency_flags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("Authorization", "Bearer test-key"))
        .and(body_partial_json(json!({
            "query": "rust async",
            "topic": "general",
            "search_depth": "basic",
            "max_results": 5,
            "include_answer": false,
            "include_raw_content": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "query": "rust async",
            "answer": "ignored",
            "responseTime": 1.1,
            "results": [
                {"title": "Async Book", "url": "https://rust-lang.github.io/async-book/",
                 "content": "Getting started", "score": 0.97, "raw_content": "ignored"}
            ]
        })))
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("test-key").with_base_url(server.uri());
    let results = provider
        .search(&SearchQuery::new("rust async").with_max_results(5))
        .await
        .expect("the call succeeds");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Async Book");
    assert_eq!(results[0].snippet, "Getting started");
}

#[tokio::test]
async fn the_news_flag_is_a_topic_not_a_route() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(
            json!({"query": "rust release", "topic": "news"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "Rust 1.90", "url": "https://blog.rust-lang.org",
                         "content": "released", "published_date": "2026-11-01"}]
        })))
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("key").with_base_url(server.uri());
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
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"detail": "unauthorized"}"#))
        .mount(&server)
        .await;
    let provider = TavilyProvider::new("bad-key").with_base_url(server.uri());
    assert_eq!(
        provider.search(&SearchQuery::new("q")).await,
        Err(SearchError::Auth)
    );
}

#[tokio::test]
async fn rate_limits_map_to_the_rate_limited_class() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let provider = TavilyProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.search(&SearchQuery::new("q")).await,
        Err(SearchError::RateLimited)
    );
}

#[tokio::test]
async fn other_statuses_carry_the_body_head() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;
    let provider = TavilyProvider::new("key").with_base_url(server.uri());
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
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;
    let provider = TavilyProvider::new("key").with_base_url(server.uri());
    let err = provider
        .search(&SearchQuery::new("q"))
        .await
        .expect_err("html is not the documented shape");
    assert!(matches!(err, SearchError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_200_without_results_is_an_honest_empty_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"query": "q"})))
        .mount(&server)
        .await;
    let provider = TavilyProvider::new("key").with_base_url(server.uri());
    let results = provider.search(&SearchQuery::new("q")).await.unwrap();
    assert!(
        results.is_empty(),
        "not a Parse error: the chain would bill a second vendor"
    );
}

#[tokio::test]
async fn a_transport_failure_is_the_transport_class() {
    // A port nothing listens on: connect refused, reqwest's own error.
    let provider = TavilyProvider::new("key").with_base_url("http://127.0.0.1:9");
    let err = provider
        .search(&SearchQuery::new("q"))
        .await
        .expect_err("nothing is listening");
    assert!(matches!(err, SearchError::Transport(_)), "{err:?}");
}

#[test]
fn the_kind_names_the_config_spelling() {
    assert_eq!(TavilyProvider::new("k").kind(), "tavily");
}

#[test]
fn the_wire_shape_round_trips_a_real_shaped_response() {
    // One field the adapter reads and three it ignores, the
    // deserializer must not choke on either.
    let response: Value = json!({
        "query": "q",
        "answer": "an llm paragraph",
        "results": [{"title": "T", "url": "https://a", "content": "c", "score": 0.5,
                     "raw_content": "the whole page"}],
        "responseTime": 0.8
    });
    let parsed: rhema_websearch_tavily::TavilyResponse = serde_json::from_value(response).unwrap();
    assert_eq!(
        rhema_websearch_tavily::results_from_response(&parsed).len(),
        1
    );
}
