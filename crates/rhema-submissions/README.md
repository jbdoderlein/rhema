# rhema-submissions

Durable identity for requested work. Every requested unit of work is a
record with an id, a kind, a payload and a status, kept inside the
session's `driver_state`, so a resumed process can tell which requests
finished, which are half done, and which never started.

```rust
let record = rhema_submissions::turn::submit(&mut session, "fix the test", at)?;
store.save(&session_id, &session)?;

let record = rhema_submissions::start(&mut session, &record.id, at, None)?;
store.save(&session_id, &session)?;

let record = rhema_submissions::settle(
    &mut session,
    &record.id,
    rhema_submissions::Outcome::Done,
    at,
)?;
store.save(&session_id, &session)?;
```

Every mutator parses the ledger, applies one transition, writes it back,
and returns. The crate saves nothing: the caller owns durability, one
`store.save` per transition, and it must not start work when that save
fails.

## Statuses and transitions

`Queued`, `Running`, `Done`, `Failed`, `Cancelled`, `Interrupted`. The
lifecycle is `submit`, `start`, `settle`, `cancel`, `retry`,
`reconcile`; a transition the table does not list is a typed error, not
a silent no-op.

A status names who owns the record, not what the work is doing. An ask
blocked on a human is still `Running`, and its payload says what it
waits for.

## The kind seam

A crate that owns a kind of work implements two methods:

```rust
pub trait Kind: Send + Sync {
    fn name(&self) -> &'static str;
    fn recover(&self, ctx: &RecoverCtx<'_>) -> Recovery;

    fn adopt(&self, ctx: &AdoptCtx<'_>) -> Vec<serde_json::Value> {
        Vec::new()
    }
}
```

`recover` answers what a `Running` record left by a dead process means
now, and there are exactly four answers: `Done`, `Queued`,
`Interrupted`, or `Failed { error }` when the work's own state is gone.
It cannot write; the lifecycle applies the verdict once per stale
record.

`adopt` covers work that started inside a tool call, where the ledger
could not be written first. The kind reads its own store, checks the
snapshot, and returns only the payloads without a record. The crate
assigns their ids and classifies them with `recover` in the same pass,
so a second load adopts nothing.

The subagent kind lives in `rhema-subagents` and reads child sessions.
The job kind lives in `rhema-agent-tools` and reads the job daemon's
listing.

Register the kinds a frontend links in a `Kinds`. A record whose kind is
not registered reconciles to `Interrupted` and is reported as
`Change::Unclaimed`: a session never refuses to load because an
extension is missing, and a record is never silently dropped.

## The built-in turn kind

`rhema_submissions::turn` is the engine's own unit of work: the prompt
is the payload, and the history is the state. `turn::drive` runs the
four steps a frontend would otherwise write (`start`, `turn`, settle,
commit), `turn::resume` continues an interrupted one without appending
the input twice, and `turn::answer` reads the reply from history.

`base` pins the history index the input landed at, so recovery never
matches prompt text: two identical prompts in a row cannot confuse it.

## The TUI and ACP

ACP records each prompt and reconciles on `session/load`. The TUI
reconciles a resumed session, restarts its jobs poller under the loaded
session's tag, and announces what it found without starting any work.

## Retention

The ledger keeps every unsettled record and the newest 32 settled ones.
The cap is a constant, because the ledger rides every commit.

Spec: `specs/rhema-submissions.md`.
