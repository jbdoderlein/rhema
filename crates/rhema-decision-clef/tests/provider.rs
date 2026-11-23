//! Offline integration tests: the provider against an in-process
//! wiremock server. Zero network.

use rhema_decision::{DecisionError, DecisionProvider, DecisionRequest, Question};
use rhema_decision_clef::{ClefModel, ClefProvider};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> DecisionRequest {
    DecisionRequest::new("Checkout has been failing for an hour.")
        .with_question("urgent", Question::noul("Is this support request urgent?"))
}

fn envelope(answers: serde_json::Value) -> serde_json::Value {
    json!({
        "result": {
            "model": "clef",
            "answers": answers,
            "usage": {"input_tokens": 120, "output_tokens": 0}
        },
        "success": true,
        "errors": [],
        "messages": []
    })
}

#[tokio::test]
async fn the_route_names_the_account_and_carries_the_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts/acc123/ai/run/@cf/cloudflare/clef"))
        .and(header("Authorization", "Bearer test-token"))
        .and(body_partial_json(json!({
            "model": "clef",
            "state": "Checkout has been failing for an hour.",
            "questions": {"urgent": {"type": "noul", "instructions": "Is this support request urgent?"}}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope(json!({"urgent": {"type": "noul", "noul": 0.9}}))),
        )
        .mount(&server)
        .await;

    let provider = ClefProvider::new("acc123", "test-token").with_base_url(server.uri());
    let response = provider
        .decide(&request())
        .await
        .expect("the call succeeds");
    assert_eq!(response.model, "clef");
    assert_eq!(response.usage.input_tokens, 120);
    assert_eq!(provider.kind(), "clef");
}

#[tokio::test]
async fn the_flash_model_takes_its_own_route_and_names_itself() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts/acc123/ai/run/@cf/cloudflare/clef-flash"))
        .and(body_partial_json(json!({"model": "clef-flash"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope(json!({"urgent": {"type": "noul", "noul": 0.1}}))),
        )
        .mount(&server)
        .await;
    let provider = ClefProvider::new("acc123", "t")
        .with_model(ClefModel::ClefFlash)
        .with_base_url(server.uri());
    assert!(provider.decide(&request()).await.is_ok());
}

#[tokio::test]
async fn a_failed_envelope_maps_through_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "success": false,
            "errors": [{"code": 10000, "message": "Authentication error"}]
        })))
        .mount(&server)
        .await;
    let provider = ClefProvider::new("acc123", "bad").with_base_url(server.uri());
    assert_eq!(provider.decide(&request()).await, Err(DecisionError::Auth));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "success": false,
            "errors": [{"code": 1001, "message": "questions.tone: 2 items required"}]
        })))
        .mount(&server)
        .await;
    let provider = ClefProvider::new("acc123", "t").with_base_url(server.uri());
    assert_eq!(
        provider.decide(&request()).await,
        Err(DecisionError::Invalid(
            "questions.tone: 2 items required".into()
        ))
    );
}

#[tokio::test]
async fn a_malformed_body_is_a_parse_error_the_chain_falls_through_on() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;
    let provider = ClefProvider::new("acc123", "t").with_base_url(server.uri());
    let err = provider
        .decide(&request())
        .await
        .expect_err("html is not the envelope");
    assert!(matches!(err, DecisionError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_transport_failure_is_the_transport_class() {
    // A port nothing listens on: connect refused, reqwest's own error.
    let provider = ClefProvider::new("acc123", "t").with_base_url("http://127.0.0.1:9/client/v4");
    let err = provider
        .decide(&request())
        .await
        .expect_err("nothing is listening");
    assert!(matches!(err, DecisionError::Transport(_)), "{err:?}");
}
