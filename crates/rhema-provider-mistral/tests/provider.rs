//! Offline tests: the chat-completions route, the models catalog, and
//! the Vibe browser sign-in, all against an in-process wiremock server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rhema_core::{
    ChatParams, Content, Event, EventSink, Message, ModelProvider, ModelRequest, Role, StopReason,
};
use rhema_provider::{InteractiveLogin, LoginSink, LoginUpdate, ModelListing};
use rhema_provider_mistral::{MistralProvider, MistralVibeLogin};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Collector(Vec<String>);

impl Collector {
    fn new() -> Self {
        Self(Vec::new())
    }
}

impl EventSink for Collector {
    fn emit(&mut self, event: Event<'_>) {
        self.0.push(format!("{event:?}"));
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        system: Some("be terse".into()),
        messages: vec![Message::text(Role::User, "hi")],
        tools: vec![],
        params: ChatParams::new("mistral-large-4-0"),
    }
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

#[tokio::test]
async fn completions_ride_chat_completions_with_the_bearer_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer vibe-key"))
        .respond_with(sse(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MistralProvider::new("vibe-key").with_base_url(server.uri());
    let mut sink = Collector::new();
    let out = provider
        .complete(request(), &mut sink)
        .await
        .expect("the chat route answered");
    assert_eq!(out.message.content, vec![Content::Text("ok".into())]);
    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert!(sink.0.iter().any(|line| line.starts_with("TextDelta")));
}

#[tokio::test]
async fn the_catalog_reads_mistral_cards_and_drops_non_chat_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer k"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "mistral-large-4-0",
                "name": "Mistral Large 4",
                "description": "A general-purpose multimodal model.",
                "capabilities": {"completion_chat": true, "vision": true},
                "max_context_length": 1048576
            },
            {
                "id": "mistral-embed-23-12",
                "capabilities": {"completion_chat": false}
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MistralProvider::new("k").with_base_url(server.uri());
    let models = provider.list_models().await.expect("the catalog answered");
    assert_eq!(models.len(), 1, "{models:?}");
    assert_eq!(models[0].id, "mistral-large-4-0");
    assert_eq!(models[0].input_token_limit, Some(1_048_576));
    assert_eq!(models[0].supports_vision, Some(true));
    assert_eq!(models[0].pricing.expect("priced").output_per_mtok, 2.10);
}

#[tokio::test]
async fn the_vibe_catalog_reads_the_model_catalog_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/model-catalog"))
        .and(header("authorization", "Bearer vibe-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [
                {
                    "id": "mistral-large-4-0",
                    "label": "Mistral Large 4",
                    "max_context_length": 1048576,
                    "capabilities": {"completion_chat": true, "vision": true}
                },
                {
                    "id": "glm-5-2",
                    "label": "GLM 5.2"
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MistralProvider::vibe("vibe-key").with_base_url(server.uri());
    let models = provider
        .list_models()
        .await
        .expect("the vibe catalog answered");
    assert_eq!(models.len(), 2, "{models:?}");
    assert_eq!(models[0].id, "glm-5-2");
    assert_eq!(models[0].display_name, "GLM 5.2");
    assert_eq!(models[1].display_name, "Mistral Large 4");
    assert_eq!(models[1].input_token_limit, Some(1_048_576));
}

#[tokio::test]
async fn a_vibe_catalog_deployment_without_the_route_falls_back_to_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/model-catalog"))
        .respond_with(ResponseTemplate::new(404).set_body_string("nope"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": "mistral-medium-3.5", "capabilities": {"completion_chat": true}}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MistralProvider::vibe("k").with_base_url(server.uri());
    let models = provider.list_models().await.expect("the fallback answered");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "mistral-medium-3.5");
}

/// A sink that keeps the updates a flow reports.
#[derive(Default)]
struct Recording {
    updates: Arc<Mutex<Vec<LoginUpdate>>>,
}

impl LoginSink for Recording {
    fn update(&mut self, update: LoginUpdate) {
        self.updates.lock().expect("not poisoned").push(update);
    }
}

fn updates(sink: &Recording) -> Vec<LoginUpdate> {
    sink.updates.lock().expect("not poisoned").clone()
}

#[tokio::test]
async fn the_vibe_login_walks_start_poll_and_exchange_to_a_key() {
    let server = MockServer::start().await;
    let sign_in_url = format!("{}/codestral/cli/authenticate#process_id=p1", server.uri());
    let poll_url = format!("{}/vibe/sign-in/poll/token-1", server.uri());

    Mock::given(method("POST"))
        .and(path("/vibe/sign-in"))
        .and(body_partial_json(serde_json::json!({
            "code_challenge_method": "S256"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "process_id": "p1",
            "sign_in_url": sign_in_url,
            "poll_url": poll_url,
            "expires_at": "2026-11-18T15:51:31Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/vibe/sign-in/poll/token-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "completed",
            "exchange_token": "exchange-1"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/vibe/sign-in/p1/exchange"))
        .and(body_partial_json(serde_json::json!({
            "exchange_token": "exchange-1"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "api_key": "vibe-key"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let flow = MistralVibeLogin::new()
        .with_auth_api_base_url(server.uri())
        .with_poll_interval(Duration::from_millis(1));
    let mut sink = Recording::default();
    flow.run(&mut sink).await;
    let updates = updates(&sink);
    assert!(
        matches!(&updates[0], LoginUpdate::Instruct(instruction) if instruction.url.as_deref() == Some(sign_in_url.as_str())),
        "{updates:?}"
    );
    assert_eq!(
        updates.last(),
        Some(&LoginUpdate::Completed("vibe-key".to_owned()))
    );
}

#[tokio::test]
async fn a_denied_vibe_login_reports_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/vibe/sign-in"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "process_id": "p1",
            "sign_in_url": format!("{}/sign-in", server.uri()),
            "poll_url": format!("{}/poll", server.uri()),
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "denied"
        })))
        .mount(&server)
        .await;
    let flow = MistralVibeLogin::new()
        .with_auth_api_base_url(server.uri())
        .with_poll_interval(Duration::from_millis(1));
    let mut sink = Recording::default();
    flow.run(&mut sink).await;
    let updates = updates(&sink);
    assert_eq!(
        updates.last(),
        Some(&LoginUpdate::Failed(
            "Mistral Vibe sign-in was denied".to_owned()
        ))
    );
}
