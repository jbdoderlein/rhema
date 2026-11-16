//! `Agent::resume_turn`: the step loop over a history whose input is
//! already committed.
#![cfg(feature = "mock")]

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rhema_core::{
    Agent, Content, Event, EventSink, Message, MockProvider, ModelOutput, ModelProvider,
    ModelRequest, ProviderError, Role, Session, StopReason, ToolCall, ToolContext, Usage,
};

struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: Event<'_>) {}
}

/// Counts the calls it forwards, so the loop guard can be read off the
/// provider's own tally.
struct Counting {
    calls: Arc<AtomicUsize>,
    inner: MockProvider,
}

#[rhema_core::async_trait]
impl ModelProvider for Counting {
    async fn complete(
        &self,
        req: ModelRequest,
        sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.complete(req, sink).await
    }
}

fn text_reply(text: &str) -> ModelOutput {
    ModelOutput {
        message: Message::text(Role::Assistant, text),
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

fn tool_reply(id: &str) -> ModelOutput {
    ModelOutput {
        message: Message {
            role: Role::Assistant,
            content: vec![Content::ToolCall(ToolCall {
                id: id.to_owned(),
                name: "echo".to_owned(),
                args: serde_json::json!({}),
                provider_data: serde_json::Value::Null,
            })],
        },
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

#[tokio::test]
async fn resume_turn_appends_nothing_and_commits_each_boundary() {
    let committed: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = committed.clone();
    let mut agent = Agent::builder()
        .provider(Arc::new(MockProvider::new(vec![text_reply("continued")])))
        .commit_hook(Box::new(move |session: &Session| {
            seen.lock().unwrap().push(session.history.len());
            Ok(())
        }))
        .build()
        .unwrap();
    let mut session = Session {
        history: vec![
            Message::text(Role::User, "first"),
            Message::text(Role::Assistant, "earlier answer"),
        ],
        ..Session::default()
    };

    let out = agent
        .resume_turn(&mut session, &mut NullSink, &ToolContext::default())
        .await
        .unwrap();

    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert_eq!(session.history.len(), 3, "only the answer was appended");
    assert_eq!(session.history[0], Message::text(Role::User, "first"));
    assert_eq!(session.history[1].role, Role::Assistant);
    assert_eq!(
        session.history[2],
        Message::text(Role::Assistant, "continued")
    );
    // The step's start boundary (before the request) and its end
    // boundary (after the answer), and no input push between them.
    assert_eq!(*committed.lock().unwrap(), [2, 3]);
}

#[tokio::test]
async fn resume_turn_honors_the_loop_guard() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Counting {
        calls: calls.clone(),
        inner: MockProvider::new(vec![
            tool_reply("call_1"),
            tool_reply("call_2"),
            tool_reply("call_3"),
        ]),
    };
    let mut agent = Agent::builder().provider(provider).build().unwrap();
    let mut session = Session {
        history: vec![Message::text(Role::User, "go")],
        ..Session::default()
    };

    let out = agent
        .resume_turn_with_limit(
            &mut session,
            &mut NullSink,
            &ToolContext::default(),
            NonZeroU32::new(2).unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(out.stop_reason, StopReason::ToolUse);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "the cap stopped the loop");
    assert_eq!(session.history.len(), 5, "two steps, four messages");
}
