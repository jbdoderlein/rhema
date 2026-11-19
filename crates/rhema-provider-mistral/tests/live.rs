//! Live smoke test against the real Mistral API. Ignored by default, CI
//! and plain `cargo test` never hit the network.
//!
//! Run with:
//!
//! ```sh
//! MISTRAL_API_KEY=... cargo test -p rhema-provider-mistral --all-features -- --ignored
//! ```

use rhema_core::{
    ChatParams, Content, Event, EventSink, Message, ModelProvider, ModelRequest, Role,
};
use rhema_provider::ModelListing;
use rhema_provider_mistral::MistralProvider;
use rhema_provider_openai_compatible::Api;

struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: Event<'_>) {}
}

fn live_key() -> String {
    std::env::var("MISTRAL_API_KEY")
        .unwrap_or_else(|_| panic!("set MISTRAL_API_KEY to run the live test"))
}

fn live_model() -> String {
    std::env::var("MISTRAL_MODEL").unwrap_or_else(|_| "mistral-large-4-0".to_owned())
}

#[tokio::test]
#[ignore = "hits the live Mistral API; set MISTRAL_API_KEY and run with `cargo test -p rhema-provider-mistral --all-features -- --ignored`"]
async fn live_mistral_lists_text_chat_models() {
    let models = MistralProvider::new(live_key())
        .list_models()
        .await
        .expect("the live models call succeeds");
    assert!(
        models.iter().any(|model| model.supports_chat()),
        "expected at least one chat model, got {models:?}"
    );
}

#[tokio::test]
#[ignore = "hits the live Mistral API; set MISTRAL_API_KEY and run with `cargo test -p rhema-provider-mistral --all-features -- --ignored`"]
async fn live_mistral_answers_a_trivial_prompt() {
    let provider = MistralProvider::new(live_key());
    let request = ModelRequest {
        system: None,
        messages: vec![Message::text(Role::User, "Reply with exactly: ok")],
        tools: vec![],
        params: ChatParams::new(live_model()),
    };
    let mut sink = NullSink;
    let out = provider
        .complete(request, &mut sink)
        .await
        .expect("the live streaming call succeeds");
    let text = out
        .message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<String>();
    assert!(
        !text.trim().is_empty(),
        "expected non-empty text, got {out:?}"
    );
}

/// The Responses API beside chat completions: the same key and base,
/// `POST {base}/responses`, the OpenAI-shaped wire their reference
/// publishes. Runs as the same smoke as the chat dialect.
///
/// Run with:
///
/// ```sh
/// MISTRAL_API_KEY=... MISTRAL_MODEL=zai-glm-5-3 cargo test -p rhema-provider-mistral --all-features -- --ignored live
/// ```
#[tokio::test]
#[ignore = "hits the live Mistral API; set MISTRAL_API_KEY to run"]
async fn live_mistral_answers_over_the_responses_route() {
    let provider = MistralProvider::new(live_key()).with_api(Api::Responses);
    let request = ModelRequest {
        system: None,
        messages: vec![Message::text(Role::User, "Reply with exactly: ok")],
        tools: vec![],
        params: ChatParams::new(live_model()),
    };
    let mut sink = NullSink;
    let out = provider
        .complete(request, &mut sink)
        .await
        .expect("the live Responses call succeeds");
    let text = out
        .message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<String>();
    assert!(
        !text.trim().is_empty(),
        "expected non-empty text, got {out:?}"
    );
}

/// The Vibe key's model catalog publishes each model's accepted
/// reasoning levels. A `medium` on a model that skips it must land on
/// one the model speaks rather than be rejected.
///
/// Run with:
///
/// ```sh
/// MISTRAL_VIBE_API_KEY=... MISTRAL_MODEL=zai-glm-5-3 ///     cargo test -p rhema-provider-mistral -- --ignored --nocapture live_vibe
/// ```
#[tokio::test]
#[ignore = "hits the live Mistral Vibe API; set MISTRAL_VIBE_API_KEY to run"]
async fn live_vibe_clamps_the_effort_and_reads_the_budget() {
    let key = std::env::var("MISTRAL_VIBE_API_KEY")
        .expect("set MISTRAL_VIBE_API_KEY to run the live Vibe test");
    let model = std::env::var("MISTRAL_MODEL").unwrap_or_else(|_| "zai-glm-5-3".to_owned());
    let provider = MistralProvider::vibe(key);

    let mut result = provider
        .allowance()
        .await
        .expect("the budget route answers");
    if result.is_none() {
        result = Some(rhema_provider::Allowance {
            amount: "(none published)".to_owned(),
            kind: String::new(),
        });
    }
    println!("vibe allowance: {:?}", result.expect("an allowance"));

    let mut req = ModelRequest {
        system: None,
        messages: vec![Message::text(Role::User, "Reply with just: ok")],
        tools: vec![],
        params: ChatParams {
            thinking: rhema_core::request::ThinkingEffort::Medium,
            ..ChatParams::new(model.clone())
        },
    };
    req.params.max_tokens = Some(5);
    let out = provider
        .complete(req, &mut NullSink)
        .await
        .expect("the clamped effort completes against the live API");
    println!("{} answered with {out:?}", model);
}
