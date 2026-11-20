//! Offline integration tests: the kinds whose endpoints speak the
//! Responses API default to it, and an openai route override keeps the
//! conservative chat-completions dialect. Every provider runs against
//! an in-process wiremock server, the request path landing there is
//! the assertion.

use rhema_core::{ChatParams, Content, Event, EventSink, Message, ModelRequest, Role, StopReason};
use rhema_provider_registry::{build, build_opencode};
use wiremock::matchers::{method, path};
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
        system: None,
        messages: vec![Message::text(Role::User, "hi")],
        tools: vec![],
        params: ChatParams::new("the-model"),
    }
}

/// A Responses-API stream: one text delta, then the terminal event.
fn responses_sse() -> String {
    [
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
    ]
    .concat()
}

/// A chat-completions stream: one chunk, then the sentinel.
fn chat_sse() -> String {
    [
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat()
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

#[tokio::test]
async fn deepseek_defaults_to_the_responses_route() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(sse(&responses_sse()))
        .mount(&server)
        .await;
    let built = build("deepseek", Some("key"), None, Some(&server.uri()), None).expect("builds");
    let mut sink = Collector::new();
    let out = built
        .provider
        .complete(request(), &mut sink)
        .await
        .expect("the Responses route answered");
    assert_eq!(
        out.message.content,
        vec![Content::Text("ok".into())],
        "the stream assembled through the Responses wire"
    );
    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert!(
        sink.0.iter().any(|line| line.starts_with("TextDelta")),
        "text streamed as a delta: {:?}",
        sink.0
    );
}

#[tokio::test]
async fn opencode_defaults_to_the_responses_route() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(sse(&responses_sse()))
        .mount(&server)
        .await;
    let built = build_opencode("key", Some(&server.uri()), None);
    let mut sink = Collector::new();
    let out = built
        .provider
        .complete(request(), &mut sink)
        .await
        .expect("the Responses route answered");
    assert_eq!(out.message.content, vec![Content::Text("ok".into())]);
    assert_eq!(out.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn an_explicit_responses_choice_lifts_the_override_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(sse(&responses_sse()))
        .mount(&server)
        .await;
    let built = build(
        "openai",
        Some("key"),
        None,
        Some(&server.uri()),
        Some("responses"),
    )
    .expect("builds");
    let mut sink = Collector(vec![]);
    let out = built
        .provider
        .complete(request(), &mut sink)
        .await
        .expect("the Responses route answered at an override URL");
    assert_eq!(
        out.message.content,
        vec![Content::Text("ok".into())],
        "the stored choice beats the conservative route rule"
    );
}

#[tokio::test]
async fn an_openai_route_override_stays_on_chat_completions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse(&chat_sse()))
        .mount(&server)
        .await;
    let built = build("openai", Some("key"), None, Some(&server.uri()), None).expect("builds");
    let mut sink = Collector::new();
    let out = built
        .provider
        .complete(request(), &mut sink)
        .await
        .expect("the chat completions route answered");
    assert_eq!(
        out.message.content,
        vec![Content::Text("ok".into())],
        "an override URL keeps the conservative dialect"
    );
    assert_eq!(out.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn both_mistral_kinds_default_to_the_responses_route() {
    // The API and the Vibe subscription share one dialect and only the
    // credential's origin differs; the deployment override the mock
    // server's URL plays still names Mistral, whose API publishes the
    // route, so the latest shape is the default there too.
    for kind in ["mistral", "mistral-vibe"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(sse(&responses_sse()))
            .expect(1)
            .mount(&server)
            .await;
        let built = build(kind, Some("key"), None, Some(&server.uri()), None).expect("builds");
        let mut sink = Collector::new();
        let out = built
            .provider
            .complete(request(), &mut sink)
            .await
            .expect("the Responses route answered");
        assert_eq!(
            out.message.content,
            vec![Content::Text("ok".into())],
            "{kind} speaks the Responses dialect by default"
        );
    }
}

#[tokio::test]
async fn an_openai_base_override_keeps_the_loosest_dialect() {
    // A base override names an unknown server, the loosest-schema
    // dialect: no cache-routing key, no session id, whatever the
    // conversation carries.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse(&chat_sse()))
        .mount(&server)
        .await;
    let built = build("openai", Some("key"), None, Some(&server.uri()), None).expect("builds");
    let mut req = request();
    req.params.session_id = Some("20261120120000-00".into());
    let mut sink = Collector::new();
    built
        .provider
        .complete(req, &mut sink)
        .await
        .expect("the chat completions route answered");
    let received = server.received_requests().await.expect("requests");
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert!(body.get("prompt_cache_key").is_none(), "{body}");
    assert!(body.get("session_id").is_none(), "{body}");
}

#[tokio::test]
async fn an_explicit_chat_choice_pins_a_mistral_instance_back() {
    // The stored escape hatch for an older deployment without the
    // route: `api: chat` keeps the dialect that always answered.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse(&chat_sse()))
        .expect(1)
        .mount(&server)
        .await;
    let built = build(
        "mistral-vibe",
        Some("key"),
        None,
        Some(&server.uri()),
        Some("chat"),
    )
    .expect("builds");
    let mut sink = Collector::new();
    let out = built
        .provider
        .complete(request(), &mut sink)
        .await
        .expect("the chat completions route answered");
    assert_eq!(
        out.message.content,
        vec![Content::Text("ok".into())],
        "the stored choice beats the Responses default"
    );
}
