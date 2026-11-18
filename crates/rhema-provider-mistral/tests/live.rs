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
