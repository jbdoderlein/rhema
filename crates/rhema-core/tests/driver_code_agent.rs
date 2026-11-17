//! Integration tests for rhema-core.
#![cfg(feature = "mock")]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use rhema_core::{
    Agent, Content, Event, EventSink, Message, MockProvider, ModelOutput, ModelProvider,
    ModelRequest, ProviderError, Role, Session, StopReason, ToolCall, ToolContext, ToolResolver,
    ToolResult, ToolSpec, Usage, run_turn,
};
use serde_json::{Value, json};

struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: Event<'_>) {}
}

/// The system prompt and roles the last request carried.
type Seen = Arc<Mutex<Option<(Option<String>, Vec<Role>)>>>;

struct CaptureProvider {
    seen: Seen,
    inner: MockProvider,
}

#[rhema_core::async_trait]
impl ModelProvider for CaptureProvider {
    async fn complete(
        &self,
        req: ModelRequest,
        sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        *self.seen.lock().unwrap() = Some((
            req.system.clone(),
            req.messages.iter().map(|message| message.role).collect(),
        ));
        self.inner.complete(req, sink).await
    }
}

struct EchoResolver;

#[rhema_core::async_trait]
impl ToolResolver for EchoResolver {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec::new(
            "echo",
            "echoes its arguments back",
            json!({ "type": "object" }),
        )]
    }

    async fn resolve(&self, call: ToolCall, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success(call.id, call.args.to_string())
    }
}

fn call(id: impl Into<String>, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,

        provider_data: serde_json::Value::Null,
    }
}

fn tool_use_output(call: ToolCall) -> ModelOutput {
    ModelOutput {
        message: Message {
            annotations: Default::default(),
            role: Role::Assistant,
            content: vec![Content::ToolCall(call)],
        },
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn end_turn_output(text: &str) -> ModelOutput {
    ModelOutput {
        message: Message::text(Role::Assistant, text),
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

#[tokio::test]
async fn step_folds_system_messages_into_the_request() {
    let seen = Arc::new(Mutex::new(None));
    let provider = CaptureProvider {
        seen: Arc::clone(&seen),
        inner: MockProvider::new(vec![end_turn_output("ok")]),
    };
    let mut agent = Agent::builder()
        .provider(provider)
        .system("base")
        .build()
        .unwrap();
    let mut session = Session::default();
    session.history.push(Message::text(Role::System, "one"));
    session.history.push(Message::text(Role::User, "hi"));
    session.history.push(Message::text(Role::System, "two"));

    agent
        .step(&mut session, &mut NullSink, &ToolContext::default())
        .await
        .unwrap();

    let (system, roles) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(system.as_deref(), Some("base\n\none\n\ntwo"));
    assert_eq!(roles, vec![Role::User]);
}

#[tokio::test]
async fn run_turn_drives_tool_use_to_a_final_answer() {
    let mut agent = Agent::builder()
        .provider(Arc::new(MockProvider::new(vec![
            tool_use_output(call("call_1", "echo", json!({ "text": "hi" }))),
            end_turn_output("all done"),
        ])))
        .resolver(Arc::new(EchoResolver))
        .build()
        .unwrap();
    let mut session = Session::default();
    let mut sink = NullSink;
    let ctx = ToolContext::default();

    let out = run_turn(
        &mut agent,
        &mut session,
        Message::text(Role::User, "run the echo tool"),
        &mut sink,
        &ctx,
        NonZeroU32::new(5).unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert_eq!(out.message, Message::text(Role::Assistant, "all done"));
    let roles: Vec<Role> = session.history.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );
    assert_eq!(
        session.history[0],
        Message::text(Role::User, "run the echo tool")
    );
    assert_eq!(
        session.history[3],
        Message::text(Role::Assistant, "all done")
    );
}

#[tokio::test]
async fn turn_takes_a_user_string_and_builds_with_concrete_seams() {
    // Both seams arrive as concrete values, not Arcs; `model` is the
    // shorthand for `params.model`; a bare string is a user message.
    let mut agent = Agent::builder()
        .provider(MockProvider::new(vec![
            tool_use_output(call("call_1", "echo", json!({ "text": "hi" }))),
            end_turn_output("all done"),
        ]))
        .resolver(EchoResolver)
        .model("mock-model")
        .build()
        .unwrap();
    let mut session = Session::default();

    let out = agent
        .turn(
            "run the echo tool",
            &mut session,
            &mut NullSink,
            &ToolContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert_eq!(out.message, Message::text(Role::Assistant, "all done"));
    assert_eq!(session.history[0], Message::user("run the echo tool"));
    let roles: Vec<Role> = session.history.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );
}

#[tokio::test]
async fn run_turn_returns_the_last_output_at_the_step_cap() {
    let mut agent = Agent::builder()
        .provider(Arc::new(MockProvider::new((0..3).map(|i| {
            tool_use_output(call(format!("call_{i}"), "echo", json!({})))
        }))))
        .resolver(Arc::new(EchoResolver))
        .build()
        .unwrap();
    let mut session = Session::default();
    let mut sink = NullSink;
    let ctx = ToolContext::default();

    let out = run_turn(
        &mut agent,
        &mut session,
        Message::text(Role::User, "loop forever"),
        &mut sink,
        &ctx,
        NonZeroU32::new(3).unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(out.stop_reason, StopReason::ToolUse);
    assert!(matches!(out.message.content[0], Content::ToolCall(_)));
    let roles: Vec<Role> = session.history.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::Tool,
        ]
    );
}
