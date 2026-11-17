//! Child sessions and the parent's `subagent` records: adoption from
//! the store, recovery from the child's own ledger, and a resume that
//! continues the tail.

use std::sync::Arc;

use async_trait::async_trait;
use rhema_agent_tools::ShellTools;
use rhema_core::{
    Content, EventSink, Message, ModelOutput, ModelProvider, ModelRequest, ProviderError, Role,
    Session, StopReason, ToolCall, ToolContext, ToolResolver, Usage,
};
use rhema_session_store::{SessionId, SessionStore};
use rhema_session_store_file::FileStore;
use rhema_subagents::kind::{self, Descriptor, Payload, SubagentKind};
use rhema_submissions::{
    Change, Kind, Kinds, Outcome, RecoverCtx, Recovery, Status, reconcile, snapshot, start, submit,
    turn,
};

/// Replies with one final answer to every request.
struct Answer;

#[async_trait]
impl ModelProvider for Answer {
    async fn complete(
        &self,
        _req: ModelRequest,
        _sink: &mut dyn EventSink,
    ) -> Result<ModelOutput, ProviderError> {
        Ok(ModelOutput {
            message: Message::text(Role::Assistant, "the rest of the work"),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        })
    }
}

fn parent_id() -> SessionId {
    SessionId::new("0000000000000001-00").unwrap()
}

fn provider() -> Arc<dyn ModelProvider> {
    Arc::new(Answer)
}

fn def() -> rhema_subagents::SubagentDef {
    rhema_subagents::SubagentDef {
        name: "worker".into(),
        description: "works".into(),
        model: None,
        max_steps: None,
        body: "Work.".into(),
    }
}

/// A child session with a descriptor and one turn record in `status`.
fn child_session(_child: &SessionId, parent: &SessionId, status: Status) -> Session {
    let mut session = Session::default();
    kind::write_descriptor(
        &mut session,
        &Descriptor {
            def: "worker".into(),
            prompt: "do the work".into(),
            model: None,
            parent: parent.clone(),
        },
    );
    let queued = turn::submit(&mut session, "do the work", 1).unwrap();
    let started = turn::start(&mut session, &queued.id, "do the work", 2).unwrap();
    session
        .history
        .push(Message::text(Role::User, "do the work"));
    match status {
        Status::Running => session.history.push(Message {
            annotations: Default::default(),
            role: Role::Assistant,
            content: vec![Content::ToolCall(ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                args: serde_json::json!({ "command": "true" }),
                ..ToolCall::default()
            })],
        }),
        Status::Done => {
            session
                .history
                .push(Message::text(Role::Assistant, "the report"));
            rhema_submissions::settle(&mut session, &started.id, Outcome::Done, 3).unwrap();
        }
        Status::Interrupted => {
            session.history.push(Message {
                annotations: Default::default(),
                role: Role::Assistant,
                content: vec![Content::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    args: serde_json::json!({ "command": "true" }),
                    ..ToolCall::default()
                })],
            });
            rhema_submissions::reconcile(&mut session, &Kinds::new(), 3).unwrap();
        }
        _ => {}
    }
    session
}

#[test]
fn a_done_child_is_adopted_and_settles_the_parent_done() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = Arc::new(FileStore::new(dir.path()));
    let parent = parent_id();
    let child = kind::child_id(store.as_ref(), &parent, 5);
    store
        .save(&child, &child_session(&child, &parent, Status::Done))
        .unwrap();
    let _ = store;

    let kinds = Kinds::new().with(Arc::new(SubagentKind::new(
        Arc::new(FileStore::new(dir.path())),
        parent.clone(),
    )));
    let mut parent_session = Session::default();
    let changes = reconcile(&mut parent_session, &kinds, 10).unwrap();

    assert_eq!(changes.len(), 1);
    let Change::Recovered {
        kind: tag,
        to: Status::Done,
        id,
    } = &changes[0]
    else {
        panic!("unexpected change: {:?}", changes[0]);
    };
    assert_eq!(tag.as_str(), "subagent");
    let after = snapshot(&parent_session).unwrap();
    let record = after.get(id).unwrap();
    let payload: Payload = serde_json::from_value(record.payload.clone()).unwrap();
    assert_eq!(payload.child, child);
    assert_eq!(payload.prompt, "do the work");
    assert_eq!(record.status, Status::Done);

    // Nothing is adopted twice.
    assert!(
        reconcile(&mut parent_session, &kinds, 11)
            .unwrap()
            .is_empty()
    );
    assert_eq!(snapshot(&parent_session).unwrap().len(), 1);
}

#[test]
fn a_running_child_with_a_working_tail_is_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = Arc::new(FileStore::new(dir.path()));
    let parent = parent_id();
    let child = kind::child_id(store.as_ref(), &parent, 5);
    store
        .save(&child, &child_session(&child, &parent, Status::Running))
        .unwrap();

    let kind = SubagentKind::new(Arc::new(FileStore::new(dir.path())), parent.clone());
    let mut parent_session = Session::default();
    submit(
        &mut parent_session,
        kind::kind(),
        serde_json::to_value(Payload {
            child: child.clone(),
            def: "worker".into(),
            prompt: "do the work".into(),
            model: None,
        })
        .unwrap(),
        1,
    )
    .unwrap();
    let id = snapshot(&parent_session)
        .unwrap()
        .newest()
        .unwrap()
        .id
        .clone();
    start(&mut parent_session, &id, 2, None).unwrap();

    let record = snapshot(&parent_session)
        .unwrap()
        .get(&id)
        .cloned()
        .unwrap();
    let child_session = store.load(&child).unwrap().unwrap();
    assert_eq!(
        kind.recover(&RecoverCtx {
            session: &child_session,
            record: &record,
        }),
        Recovery::Interrupted
    );

    let changes = reconcile(
        &mut parent_session,
        &Kinds::new().with(Arc::new(SubagentKind::new(
            Arc::new(FileStore::new(dir.path())),
            parent,
        ))),
        10,
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        snapshot(&parent_session).unwrap().get(&id).unwrap().status,
        Status::Interrupted
    );
}

#[test]
fn a_missing_child_settles_failed_and_never_retries() {
    let dir = tempfile::tempdir().unwrap();
    let parent = parent_id();
    let kind = SubagentKind::new(Arc::new(FileStore::new(dir.path())), parent);
    let mut session = Session::default();
    submit(
        &mut session,
        kind::kind(),
        serde_json::to_value(Payload {
            child: SessionId::new("0000000000000001-00.sub-0000000000000009").unwrap(),
            def: "worker".into(),
            prompt: "do the work".into(),
            model: None,
        })
        .unwrap(),
        1,
    )
    .unwrap();
    let id = snapshot(&session).unwrap().newest().unwrap().id.clone();
    start(&mut session, &id, 2, None).unwrap();

    let changes = reconcile(&mut session, &Kinds::new().with(Arc::new(kind)), 3).unwrap();
    assert_eq!(changes.len(), 1);
    let record = snapshot(&session).unwrap().get(&id).cloned().unwrap();
    assert_eq!(record.status, Status::Failed);
    assert!(matches!(
        record.outcome,
        Some(Outcome::Failed {
            retryable: false,
            ..
        })
    ));
    assert!(
        rhema_submissions::retry(&mut session, &id, 4).is_err(),
        "a missing child cannot be retried"
    );
}

#[tokio::test]
async fn a_resumed_child_continues_its_tail_and_settles_both_records() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = Arc::new(FileStore::new(dir.path()));
    let parent = parent_id();
    let child = kind::child_id(store.as_ref(), &parent, 5);
    store
        .save(&child, &child_session(&child, &parent, Status::Running))
        .unwrap();

    let tools = rhema_subagents::SubagentTools::new(
        provider(),
        vec![def()],
        Arc::new(ShellTools::new(dir.path())),
        "test-model",
    );
    tools.attach_store(store.clone(), parent.clone());
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        run_id: "turn-1".into(),
        ..ToolContext::default()
    };

    let report = tools.resume(&child, &ctx).await.expect("the tail resumes");
    assert_eq!(report, "the rest of the work");

    // The child's ledger and tail both say done, and the parent's pass
    // reads the child rather than any copy.
    let resumed = store.load(&child).unwrap().unwrap();
    let newest = snapshot(&resumed)
        .unwrap()
        .newest()
        .cloned()
        .expect("the child keeps its turn record");
    assert_eq!(newest.status, Status::Done);
    assert!(
        resumed
            .history
            .last()
            .is_some_and(|message| message.role == Role::Assistant)
    );

    let mut parent_session = Session::default();
    let changes = reconcile(
        &mut parent_session,
        &Kinds::new().with(Arc::new(SubagentKind::new(store, parent))),
        10,
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    let record = snapshot(&parent_session).unwrap().newest().unwrap().clone();
    assert_eq!(record.status, Status::Done);
    assert_eq!(
        kind::report(&resumed).as_deref(),
        Some("the rest of the work")
    );
}

#[tokio::test]
async fn a_background_run_writes_a_child_session_and_announces_it() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = Arc::new(FileStore::new(dir.path()));
    let parent = parent_id();
    let tools = Arc::new(rhema_subagents::SubagentTools::new(
        provider(),
        vec![def()],
        Arc::new(ShellTools::new(dir.path())),
        "test-model",
    ));
    tools.attach_store(store.clone(), parent.clone());
    let handle = tools.handle();

    let result = ToolResolver::resolve(
        tools.as_ref(),
        ToolCall {
            id: "c1".into(),
            name: "subagent".into(),
            args: serde_json::json!({ "name": "worker", "prompt": "do the work" }),
            provider_data: serde_json::Value::Null,
        },
        &ToolContext {
            cwd: dir.path().to_path_buf(),
            run_id: "turn-1".into(),
            ..ToolContext::default()
        },
    )
    .await;
    assert!(!result.is_error, "{result:?}");

    // The start named its child before the first model call, and the
    // frontend can drain it into the parent's ledger.
    let started = handle.take_starts();
    assert_eq!(started.len(), 1);
    let child = started[0].child.clone();
    assert!(kind::is_child(&child));
    assert!(kind::is_child_of(&parent, &child));
    assert!(handle.take_starts().is_empty(), "a drain empties the list");

    // Wait for the run to settle into the store.
    for _ in 0..200 {
        if store
            .load(&child)
            .unwrap()
            .is_some_and(|session| kind::finished(&session))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let session = store.load(&child).unwrap().expect("the child was saved");
    let descriptor = kind::descriptor(&session).expect("the descriptor was written");
    assert_eq!(descriptor.parent, parent);
    assert_eq!(descriptor.prompt, "do the work");
    assert_eq!(
        kind::report(&session).as_deref(),
        Some("the rest of the work")
    );
    assert_eq!(
        snapshot(&session).unwrap().newest().unwrap().status,
        Status::Done
    );

    // The parent rebuilds the same record from the child.
    let mut parent_session = Session::default();
    reconcile(
        &mut parent_session,
        &Kinds::new().with(Arc::new(SubagentKind::new(store, parent))),
        10,
    )
    .unwrap();
    let record = snapshot(&parent_session).unwrap().newest().unwrap().clone();
    assert_eq!(record.status, Status::Done);
    let payload: Payload = serde_json::from_value(record.payload).unwrap();
    assert_eq!(payload.child, child);
}

#[tokio::test]
async fn a_finished_child_fetches_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = Arc::new(FileStore::new(dir.path()));
    let parent = parent_id();
    let child = kind::child_id(store.as_ref(), &parent, 5);
    store
        .save(&child, &child_session(&child, &parent, Status::Done))
        .unwrap();

    // A fresh process: no registry memory, only the store.
    let tools = rhema_subagents::SubagentTools::new(
        provider(),
        vec![def()],
        Arc::new(ShellTools::new(dir.path())),
        "test-model",
    );
    tools.attach_store(store, parent);
    let result = tools
        .resolve(
            ToolCall {
                id: "c1".into(),
                name: "subagent".into(),
                args: serde_json::json!({ "id": child.as_str() }),
                ..ToolCall::default()
            },
            &ToolContext {
                cwd: dir.path().to_path_buf(),
                ..ToolContext::default()
            },
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "the report");
}
