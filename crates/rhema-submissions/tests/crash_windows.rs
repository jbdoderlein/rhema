//! The turn kind's crash rules: what a cut session means on the next load.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use rhema_core::{
    Agent, CommitError, Content, Event, EventSink, Message, MockProvider, ModelOutput,
    ModelProvider, ModelRequest, ProviderError, Role, Session, StopReason, ToolCall, ToolContext,
    Usage,
};
use rhema_submissions::{
    Cancel, Kinds, Outcome, Status, SubmissionId, cancel, reconcile, settle, snapshot, start, turn,
};
use serde_json::json;

struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: Event<'_>) {}
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
                name: "bash".to_owned(),
                args: json!({ "command": "true" }),
                provider_data: serde_json::Value::Null,
            })],
        },
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn agent(replies: Vec<ModelOutput>) -> Agent {
    Agent::builder()
        .provider(Arc::new(MockProvider::new(replies)))
        .build()
        .unwrap()
}

/// The kinds a driver links: the built-in turn kind, nothing else.
fn kinds() -> Kinds {
    Kinds::new().with(Arc::new(turn::Turn))
}

/// A `Running` turn record over `history`, with the payload the test
/// wants.
fn stale(payload: serde_json::Value, history: Vec<Message>) -> (Session, SubmissionId) {
    let mut session = Session {
        history,
        ..Session::default()
    };
    let queued = turn::submit(&mut session, "go", 1).unwrap();
    start(&mut session, &queued.id, 2, Some(payload)).unwrap();
    (session, queued.id)
}

/// What the driver does next with a reconciled record.
fn next_step(status: Status, session: &Session, id: &SubmissionId) -> String {
    match status {
        Status::Queued => "turn".to_owned(),
        Status::Done => format!(
            "answer: {}",
            turn::answer(session, &snapshot(session).unwrap(), id).unwrap_or_default()
        ),
        Status::Interrupted => "resume".to_owned(),
        other => panic!("a reconciled turn never lands {other}"),
    }
}

#[test]
fn the_five_recovery_rules_hold_row_by_row() {
    let input = Message::text(Role::User, "go");
    // label, payload, history, status, answer, next step
    type Case<'a> = (
        &'a str,
        serde_json::Value,
        Vec<Message>,
        Status,
        Option<&'a str>,
        &'a str,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "base missing",
            json!({ "input": "go" }),
            vec![input.clone()],
            Status::Interrupted,
            None,
            "resume",
        ),
        (
            "history shorter than base",
            json!({ "input": "go", "base": 5 }),
            vec![input.clone()],
            Status::Interrupted,
            None,
            "resume",
        ),
        (
            "the input never landed",
            json!({ "input": "go", "base": 1 }),
            vec![input.clone()],
            Status::Queued,
            None,
            "turn",
        ),
        (
            "another message sits at base",
            json!({ "input": "go", "base": 0 }),
            vec![
                Message::text(Role::User, "something else"),
                Message::text(Role::Assistant, "unrelated"),
            ],
            Status::Interrupted,
            None,
            "resume",
        ),
        (
            "the answer is committed",
            json!({ "input": "go", "base": 0 }),
            vec![input.clone(), Message::text(Role::Assistant, "done")],
            Status::Done,
            Some("done"),
            "answer: done",
        ),
        (
            "the input landed, nothing after it",
            json!({ "input": "go", "base": 0 }),
            vec![input.clone()],
            Status::Interrupted,
            None,
            "resume",
        ),
        (
            "a tool call is unanswered",
            json!({ "input": "go", "base": 0 }),
            vec![
                input.clone(),
                tool_reply("call-1").message,
                Message {
                    role: Role::Tool,
                    content: vec![Content::ToolResult(rhema_core::ToolResult::success(
                        "call-1", "ran",
                    ))],
                },
            ],
            Status::Interrupted,
            None,
            "resume",
        ),
    ];

    for (label, payload, history, expected, answer, next) in cases {
        let (mut session, id) = stale(payload, history);
        let changes = reconcile(&mut session, &kinds(), 10).unwrap();
        assert_eq!(changes.len(), 1, "{label}: one stale record");
        let record = snapshot(&session).unwrap();
        let record = record.get(&id).unwrap();
        assert_eq!(record.status, expected, "{label}: status");
        assert_eq!(
            turn::answer(&session, &snapshot(&session).unwrap(), &id).as_deref(),
            answer,
            "{label}: answer"
        );
        assert_eq!(
            next_step(record.status, &session, &id),
            next,
            "{label}: next step"
        );
    }
}

#[tokio::test]
async fn drive_cut_at_each_commit_reconciles_to_the_right_next_step() {
    let mut agent = agent(vec![tool_reply("call-1"), text_reply("the answer")]);
    let mut session = Session::default();
    let record = turn::submit(&mut session, "run it", 1).unwrap();

    // Every commit the drive makes, in order: the start, the agent's
    // three boundaries per step, and the settle.
    let snapshots: Arc<Mutex<Vec<Session>>> = Arc::new(Mutex::new(Vec::new()));
    // A frontend wires the engine's hook and the lifecycle's commit to
    // one save; the test collects what that save would have written.
    let hooked = snapshots.clone();
    agent.set_commit_hook(Some(Box::new(move |session: &Session| {
        hooked.lock().unwrap().push(session.clone());
        Ok(())
    })));
    let collected = snapshots.clone();
    let mut commit = move |session: &Session| {
        collected.lock().unwrap().push(session.clone());
        Ok(())
    };
    turn::drive(
        &mut agent,
        &mut session,
        &record.id,
        &mut NullSink,
        &ToolContext::default(),
        &mut commit,
    )
    .await
    .unwrap();

    let snapshots = snapshots.lock().unwrap();
    // start, step1-start, step1-tool-use, step1-end, step2-start,
    // step2-end, settle.
    assert_eq!(snapshots.len(), 7, "the commit points, in order");
    let expected = [
        (0, Status::Queued, "turn"),
        (1, Status::Interrupted, "resume"),
        (2, Status::Interrupted, "resume"),
        (3, Status::Interrupted, "resume"),
        (3, Status::Interrupted, "resume"),
        (4, Status::Done, "answer: the answer"),
        (4, Status::Done, "answer: the answer"),
    ];
    for (index, (cut, (expected_len, expected_status, expected_next))) in
        snapshots.iter().zip(expected).enumerate()
    {
        assert_eq!(cut.history.len(), expected_len, "cut {index}: history");
        let mut cut = cut.clone();
        let before = snapshot(&cut).unwrap().get(&record.id).cloned().unwrap();
        if before.status == Status::Running {
            let changes = reconcile(&mut cut, &kinds(), 10).unwrap();
            assert_eq!(changes.len(), 1, "cut {index}: one stale record");
        }
        let after = snapshot(&cut).unwrap();
        let record = after.get(&record.id).unwrap();
        assert_eq!(record.status, expected_status, "cut {index}: status");
        assert_eq!(
            next_step(record.status, &cut, &record.id),
            expected_next,
            "cut {index}: next step"
        );
    }
}

#[test]
fn answer_is_none_after_a_compaction_shortened_the_history() {
    let (mut session, id) = stale(
        json!({ "input": "go", "base": 10 }),
        vec![
            Message::text(Role::User, "go"),
            Message::text(Role::Assistant, "done"),
        ],
    );
    settle(&mut session, &id, Outcome::Done, 3).ok();
    let after = snapshot(&session).unwrap();
    assert_eq!(turn::answer(&session, &after, &id), None);
}

#[tokio::test]
async fn a_turn_stopped_by_the_step_cap_is_done() {
    let mut agent = agent(vec![tool_reply("call-1"), text_reply("unreachable")]);
    let mut session = Session::default();
    let record = turn::submit(&mut session, "go", 1).unwrap();
    turn::start(&mut session, &record.id, "go", 2).unwrap();
    let out = agent
        .turn_with_limit(
            "go",
            &mut session,
            &mut NullSink,
            &ToolContext::default(),
            NonZeroU32::new(1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.stop_reason, StopReason::ToolUse);

    let settled = settle(&mut session, &record.id, Outcome::Done, 3).unwrap();
    assert_eq!(settled.status, Status::Done);
    assert_eq!(
        settled.outcome,
        Some(Outcome::Done),
        "a cap hit is a finished turn, not a failure"
    );
    assert_eq!(
        turn::answer(&session, &snapshot(&session).unwrap(), &record.id),
        None,
        "the last assistant message asked for tools, so it carries no answer"
    );
}

struct DeadLink;

#[rhema_core::async_trait]
impl ModelProvider for DeadLink {
    async fn complete(
        &self,
        _req: ModelRequest,
        _sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        Err(ProviderError::Network("dead link".into()))
    }
}

fn ok_commit(_session: &Session) -> Result<(), CommitError> {
    Ok(())
}

#[tokio::test]
async fn a_failure_is_classed_at_settle_time() {
    // A transport death is retryable; the core error's own verdict
    // rides into the record.
    let mut dead = Agent::builder().provider(DeadLink).build().unwrap();
    let mut session = Session::default();
    let record = turn::submit(&mut session, "go", 1).unwrap();
    let mut commit = ok_commit;
    let settled = turn::drive(
        &mut dead,
        &mut session,
        &record.id,
        &mut NullSink,
        &ToolContext::default(),
        &mut commit,
    )
    .await
    .unwrap();
    assert_eq!(settled.status, Status::Failed);
    assert!(matches!(
        settled.outcome,
        Some(Outcome::Failed {
            retryable: true,
            ..
        })
    ));

    // A commit hook that cannot write is permanent: the same failure
    // repeats identically.
    let mut refusing = agent(vec![text_reply("unreachable")]);
    refusing.set_commit_hook(Some(Box::new(|_| Err(CommitError("disk full".into())))));
    let mut session = Session::default();
    let record = turn::submit(&mut session, "go", 1).unwrap();
    let mut commit = ok_commit;
    let settled = turn::drive(
        &mut refusing,
        &mut session,
        &record.id,
        &mut NullSink,
        &ToolContext::default(),
        &mut commit,
    )
    .await
    .unwrap();
    assert_eq!(settled.status, Status::Failed);
    assert!(matches!(
        settled.outcome,
        Some(Outcome::Failed {
            retryable: false,
            ..
        })
    ));
}

#[tokio::test]
async fn resume_continues_an_interrupted_turn_without_replaying_the_input() {
    // The input and a refused tool call are on disk; the next process
    // resumes the tail and appends only the answer.
    let input = Message::text(Role::User, "go");
    let (mut session, id) = stale(
        json!({ "input": "go", "base": 0 }),
        vec![input.clone(), tool_reply("call-1").message],
    );
    reconcile(&mut session, &kinds(), 10).unwrap();
    assert_eq!(
        snapshot(&session).unwrap().get(&id).unwrap().status,
        Status::Interrupted
    );

    let mut agent = agent(vec![text_reply("resumed")]);
    let mut commit = ok_commit;
    let settled = turn::resume(
        &mut agent,
        &mut session,
        &id,
        &mut NullSink,
        &ToolContext::default(),
        &mut commit,
    )
    .await
    .unwrap();

    assert_eq!(settled.status, Status::Done);
    assert_eq!(settled.attempts, 2, "the retry counted the attempt");
    assert_eq!(
        session
            .history
            .iter()
            .filter(|message| message.role == Role::User)
            .count(),
        1,
        "the input was not replayed"
    );
    assert_eq!(
        turn::answer(&session, &snapshot(&session).unwrap(), &id).as_deref(),
        Some("resumed")
    );
    // The dangling call was answered before the model was asked again.
    assert!(session.history.iter().any(|message| matches!(
        message.content.first(),
        Some(Content::ToolResult(result)) if result.id == "call-1" && result.is_error
    )));
}

#[test]
fn a_cancelled_turn_stays_cancelled_on_the_next_load() {
    let mut session = Session::default();
    let record = turn::submit(&mut session, "go", 1).unwrap();
    start(&mut session, &record.id, 2, None).unwrap();

    // A live owner: cancelling is not a write.
    assert!(matches!(
        cancel(&mut session, &record.id, 3).unwrap(),
        Cancel::InFlight
    ));
    // The driver stops its own work and settles.
    let settled = settle(&mut session, &record.id, Outcome::Cancelled, 4).unwrap();
    assert_eq!(settled.status, Status::Cancelled);

    // The next load has nothing to reconcile.
    let before = session.driver_state.clone();
    assert!(reconcile(&mut session, &kinds(), 5).unwrap().is_empty());
    assert_eq!(session.driver_state, before);
}
