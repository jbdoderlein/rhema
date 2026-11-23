//! Offline integration tests: the provider against an in-process
//! wiremock server. Zero network.

use rhema_decision::{DecisionError, DecisionProvider, DecisionRequest, Question};
use rhema_decision_jev::JevProvider;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> DecisionRequest {
    DecisionRequest::new("I was charged twice. Please help.")
        .with_question("billing", Question::noul("Is this message about billing?"))
}

#[tokio::test]
async fn the_route_carries_the_bearer_key_and_the_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("Authorization", "Bearer test-key"))
        .and(body_partial_json(json!({
            "model": "jev-latest",
            "state": "I was charged twice. Please help.",
            "questions": {"billing": {"type": "noul", "instructions": "Is this message about billing?"}}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"billing": {"type": "noul", "noul": 0.98}},
            "usage": {"input_tokens": 120, "output_tokens": 12}
        })))
        .mount(&server)
        .await;

    let provider = JevProvider::new("test-key").with_base_url(server.uri());
    let response = provider
        .decide(&request())
        .await
        .expect("the call succeeds");
    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!(provider.kind(), "jev");
    assert_eq!(response.usage.input_tokens, 120);
}

#[tokio::test]
async fn a_pinned_model_rides_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_partial_json(json!({"model": "jev-preview"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-preview",
            "answers": {"billing": {"type": "noul", "noul": 0.5}},
            "usage": {"input_tokens": 1, "output_tokens": 0}
        })))
        .mount(&server)
        .await;
    let provider = JevProvider::new("k")
        .with_model("jev-preview")
        .with_base_url(server.uri());
    assert_eq!(
        provider.decide(&request()).await.unwrap().model,
        "jev-preview"
    );
}

#[tokio::test]
async fn auth_and_rate_limits_map_to_their_classes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"detail": "bad key"})))
        .mount(&server)
        .await;
    let provider = JevProvider::new("bad-key").with_base_url(server.uri());
    assert_eq!(provider.decide(&request()).await, Err(DecisionError::Auth));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let provider = JevProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.decide(&request()).await,
        Err(DecisionError::RateLimited)
    );
}

#[tokio::test]
async fn a_refused_request_is_invalid_with_its_detail() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "detail": "choice question 'team' needs at least 2 options"
        })))
        .mount(&server)
        .await;
    let provider = JevProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.decide(&request()).await,
        Err(DecisionError::Invalid(
            "choice question 'team' needs at least 2 options".into()
        ))
    );
}

#[tokio::test]
async fn other_statuses_carry_the_body_head() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;
    let provider = JevProvider::new("key").with_base_url(server.uri());
    assert_eq!(
        provider.decide(&request()).await,
        Err(DecisionError::Http {
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
    let provider = JevProvider::new("key").with_base_url(server.uri());
    let err = provider
        .decide(&request())
        .await
        .expect_err("html is not the documented shape");
    assert!(matches!(err, DecisionError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_transport_failure_is_the_transport_class() {
    // A port nothing listens on: connect refused, reqwest's own error.
    let provider = JevProvider::new("key").with_base_url("http://127.0.0.1:9");
    let err = provider
        .decide(&request())
        .await
        .expect_err("nothing is listening");
    assert!(matches!(err, DecisionError::Transport(_)), "{err:?}");
}
