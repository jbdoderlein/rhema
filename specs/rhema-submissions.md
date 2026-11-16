# rhema-submissions, durable identity for requested work

> **Status:** v1, 2026-11-16, reviewed. Nothing implemented yet.
> **Framework grounding:** SPEC.md §6 (the `Session` is the durable record
> and the driver checkpoints it at step boundaries),
> `rhema_core::CommitHook` (the engine's persistence seam),
> `Session::driver_state` (core's blessed place for driver state),
> `specs/rhema-session-store.md` (the store this layer rides on), and
> `specs/rhema-subagents.md` (the first consumer that is not a turn).
> **One-line promise:** every requested unit of work has a durable id, a
> kind, a payload and a status; a resumed process can always tell which
> requests finished, which are half done, and which never started, and
> the crate that owns a kind decides what "finished" means for it.

---

## 0. Scope

`rhema-submissions` is the **lifecycle layer**. It owns one record shape
inside `Session::driver_state`, the state machine over it, and a seam
through which any crate teaches it how to read a crash. It knows the
engine's turn because the turn is the engine's unit of work; it knows no
other kind.

**In v1:**

- `Record`, `Kind`, `Status`, `Outcome`, the `submissions` key, and the
  `Snapshot` plus mutator API over `&mut Session`.
- The lifecycle: `submit`, `start`, `settle`, `cancel`, `retry`,
  `reconcile`.
- The kind seam: `Kind`, `RecoverCtx`, `Recovery`, `Kinds`, and the
  unknown-kind rule.
- One built-in kind, `turn`, because `rhema_core::Agent::turn` is the
  unit every frontend already has (§6).
- `Agent::resume_turn` in `rhema-core` (§9), so an interrupted turn
  continues instead of duplicating its input.
- ACP adoption: `session/prompt` records a turn submission, a dropped
  connection settles it, `session/load` reconciles.

**OUT of v1** (each deliberately out, with the reason):

- ❌ **A closed set of kinds.** §1 and §5. The crate ships one, the
  seam has no list, and a kind is a string tag plus a trait impl in the
  crate that owns it.
- ❌ **Subagent, job, and ask kinds.** Their crates implement the seam
  when they land (§10). The schema already carries everything they need:
  a tag, an opaque payload, and a recovery callback.
- ❌ **A submissions table or a second store.** §2.
- ❌ **Leases, heartbeats, CAS, cross-process attach.** One process per
  session is today's rule (the store is last-writer-wins). A lease
  without CAS is a promise the store cannot keep.
- ❌ **A `Waiting` status.** A status names who owns a record, not what
  the work is doing. An ask blocked on the user is still owned by a live
  process, so it is `Running`, and its payload says what it waits for.
- ❌ **A queue.** v1 runs one submission at a time in submission order.
- ❌ **Auto-retry.** A terminal record stays terminal until a driver
  calls `retry`; the crate never retries on its own.
- ❌ **Submissions in the TUI and headless.** §10.

---

## 1. What a submission is

A submission is one request for work, with an id a client can hold
across a disconnect. It is three things the framework can keep without
understanding the work:

1. **Identity and payload.** Who asked for what, durably, before any
   side effect.
2. **Status.** Queued, running, or one of four terminal outcomes.
3. **A kind.** A tag naming the crate that owns the work, plus that
   crate's promise that it can read a crash.

It is not a second copy of the work. A turn's state is the session
history; a job's state is the supervisor's; a subagent's is its child
session. The submission is the **index**: an id, a kind, a request, and
where in its life it is.

The kinds are open, and each one is small:

| Kind | Lives in | Payload | State lives in | Recovery |
|---|---|---|---|---|
| `turn` | this crate | the prompt | session history | read the history tail |
| `subagent` | `rhema-subagents` | the child's id | the child session | recurse into it |
| `job` | the `job` wiring | command, backend id | the daemon | ask the daemon |
| `ask` | future crate | the question | UI pending state | `Interrupted` (no resume) |

Nothing in the list is in this crate's code. The table is illustrative;
the seam does not change when a new row appears.

What every kind promises the framework in return:

- it has a stable name, so a record written by a build that links the
  kind still loads in a build that does not, as an unclaimed record
  (§5);
- it implements `recover`, the one function that answers "a process
  died holding this record; what does the world say now?";
- its payload is its own schema, versioned by its own crate.

---

## 2. Where it lives, and why not a table

The records live in `Session::driver_state`:

```json
{
  "submissions": {
    "v": 1,
    "items": {
      "0001759000000000-00": {
        "kind": "turn",
        "payload": { "input": "fix the test", "base": 42 },
        "status": "running"
      }
    }
  }
}
```

`driver_state` is core's contract: opaque to the engine, written by the
driver, checkpointed with the session (SPEC.md §6, `session.rs`). The
`submissions` key is reserved for this crate; another driver that wants
`driver_state` for something else (RSD's key/plan) uses another session,
and the store stays the one thing that touches bytes.

The alternative, a `submissions` table beside `sessions` in
`rhema-session-store-sqlite`, fails on the write the feature exists for:

- When the work's state is the session (a turn), the answer lands in
  `history` and the status would land in the table: two records, two
  writes, and the file backend cannot span them in one transaction. The
  store would grow a transaction API and a second record type for every
  backend.
- `driver_state` already is that transaction: the store writes the
  whole session, so the answer and the status travel in one record, and
  a crash between them is reconciled from the same file on every
  backend, with no new trait method.

The one-write rule is therefore **conditional on where the work's state
lives**, and the spec says so instead of pretending otherwise:

- Work whose state is the session (a turn, a child session later): one
  write, and the crash window between answer and status is narrow and
  recoverable from the same file (§7).
- Work whose state is elsewhere (a daemon, a pending question): two
  stores, two clocks, and recovery is where they are reconciled. The
  record is still the durable index; a status may trail the daemon by
  one poll, and that is the kind's business.

The cost is real and accepted: `driver_state` is rewritten with the
session on every commit, so the ledger is serialized on every step
boundary. Retention (§4) keeps it under a few hundred kilobytes, which
is noise against the 2 MB to 15 MB sessions in the audit.

---

## 3. The record

```rust
/// Milliseconds since the epoch, passed in so tests never sleep.
pub type Millis = u64;

/// One request and its life.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The id the caller holds; also the key in `items`.
    pub id: SubmissionId,
    /// The tag of the crate that owns the work.
    pub kind: KindName,
    /// The kind's own request, opaque to this crate.
    pub payload: serde_json::Value,
    pub status: Status,
    pub created_at: Millis,
    pub started_at: Option<Millis>,
    pub settled_at: Option<Millis>,
    /// Which process claimed it. Diagnostics only in v1 (§8).
    pub owner: Option<Owner>,
    /// How it ended, when it did.
    pub outcome: Option<Outcome>,
    /// How many times it was started; 1 for a normal life.
    pub attempts: u32,
}

/// A kind tag: `[a-z0-9][a-z0-9-]{0,31}`, validated at construction.
/// Not an enum. Any crate may mint one.
pub struct KindName(String);

impl KindName {
    pub fn new(name: &str) -> Result<Self, SubmissionError>;
    pub fn as_str(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted and durable, no work started.
    Queued,
    /// A live owner has it: working, or blocked on something it is
    /// waiting for. The kind's payload says which.
    Running,
    /// The kind says the work finished.
    Done,
    /// The kind says the work ended in an error.
    Failed,
    /// The driver cancelled it before it finished.
    Cancelled,
    /// A `Running` record found by a process that did not start it, and
    /// whose kind could not say it finished.
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Finished. The result is the kind's to derive; the record does
    /// not copy it (§6).
    Done,
    /// The work ended in an error. `retryable` is the driver's verdict,
    /// taken at settle time when it still has the failure in hand: a
    /// network death is retryable, a spent budget or a failed test is
    /// not. The lifecycle uses it to decide whether `retry` may start
    /// the record again. A turn passes `AgentError::is_retryable`,
    /// which core already defines for exactly this caller.
    Failed { error: String, retryable: bool },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner { pub pid: u32 }
```

- `SubmissionId` is `rhema_session_store::SessionId`: one validated
  alphabet in the framework, and an id that is already safe to name a
  child session with.
- Every mutator takes `at: Millis`; tests pass fixed values.
- `payload` is the kind's schema. The crate reads it only to hand it
  back; `RecoverCtx::payload::<T>()` is the typed accessor.
- `Outcome::Done` carries no result, for the same reason the record
  carries no answer: a job's result is in the daemon, a turn's in
  history, a subagent's in the child session. Copying it would put
  every past result into every future commit and let the copy drift
  from the source.

**Retention.** On every mutation, the ledger keeps every unsettled
record and the newest 32 settled ones, dropping the oldest settled
first. The cap is a constant, not configuration.

**Versioning.** `v` must be `1`. A different version is
`Err(SubmissionError::Unsupported(version))` and the driver must not
start work: a newer harness wrote a shape this one would corrupt. A
missing key, `null` `driver_state`, or a `driver_state` holding other
keys all read as an empty ledger, and an empty ledger is never written
back until the first `submit`, so `--save` files stay as they are.

---

## 4. The lifecycle

```
              submit                 start                 settle
   (none) ───────────▶ Queued ───────────▶ Running ───────────▶ Done
                         │                  │  │                Failed
                         │ cancel           │  └── error ─────▶ Cancelled
                         ▼                  │
                     Cancelled              └─ reconcile (process gone)
                                              ▼
                                          Interrupted ── retry ──▶ Running
                                          Failed ─────── retry ──▶ Running
```

| From | Event | To | Writes |
|---|---|---|---|
| none | `submit` | `Queued` | record, saved by the caller |
| `Queued` | `start` | `Running` | `started_at`, `owner`, `attempts`, payload |
| `Queued` | `cancel` | `Cancelled` | `settled_at` |
| `Running` | `settle(Done)` | `Done` | `outcome`, `settled_at` |
| `Running` | `settle(Failed)` | `Failed` | `outcome`, `settled_at` |
| `Running` | `settle(Cancelled)` | `Cancelled` | `settled_at` |
| `Running` | `reconcile` → `Done` | `Done` | `outcome`, `settled_at` |
| `Running` | `reconcile` → `Queued` | `Queued` | `settled_at = None` |
| `Running` | `reconcile` → `Interrupted` | `Interrupted` | `settled_at` |
| `Interrupted` | `retry` | `Running` | `settled_at = None`, `attempts += 1` |
| `Failed` (retryable) | `retry` | `Running` | `outcome` and `settled_at` cleared |
| `Interrupted` | `cancel` | `Cancelled` | `settled_at` |

Any transition the table does not list is
`Err(SubmissionError::Transition { id, from, to })`, never a silent
no-op; `reconcile` is the exception, since it ignores terminal records
by design. The state machine is this crate's whole job; the engine, the
kinds and the frontends stay ignorant of it.

**Cancel of a `Running` record** is not a write. The work is in the
driver's task (or a daemon); only the driver can stop it. `cancel`
returns `Cancel::InFlight`, the driver aborts and settles, and a cancel
that races a finishing turn loses to the settle that already wrote
`Done`.

**`start` may re-seat the payload** (`Some(value)`), which is how a kind
records state it only knows at start time. The built-in turn kind uses
it to pin the history index its input will land at (§6). It is not a
general edit channel: the payload is written with the transition, never
between them.

### The API

```rust
/// A parsed, owned view of the ledger.
pub struct Snapshot { /* version + records, oldest first by id */ }

impl Snapshot {
    pub fn get(&self, id: &SubmissionId) -> Option<&Record>;
    /// The queued or running record a driver should pick up, if any.
    pub fn active(&self) -> Option<&Record>;
    /// The newest record, settled or not.
    pub fn newest(&self) -> Option<&Record>;
    pub fn iter(&self) -> impl Iterator<Item = &Record>;
}

pub fn snapshot(session: &Session) -> Result<Snapshot, SubmissionError>;

pub fn next_id(session: &Session, at: Millis) -> Result<SubmissionId, SubmissionError>;
pub fn submit(session: &mut Session, kind: KindName, payload: serde_json::Value, at: Millis)
    -> Result<Record, SubmissionError>;
pub fn start(session: &mut Session, id: &SubmissionId, at: Millis,
             payload: Option<serde_json::Value>) -> Result<Record, SubmissionError>;
pub fn settle(session: &mut Session, id: &SubmissionId, outcome: Outcome, at: Millis)
    -> Result<Record, SubmissionError>;
pub fn cancel(session: &mut Session, id: &SubmissionId, at: Millis)
    -> Result<Cancel, SubmissionError>;
pub fn retry(session: &mut Session, id: &SubmissionId, at: Millis)
    -> Result<Record, SubmissionError>;
pub fn reconcile(session: &mut Session, kinds: &Kinds, at: Millis)
    -> Result<Vec<Change>, SubmissionError>;

pub enum Cancel { Settled(Record), InFlight }

/// What `reconcile` did, one entry per stale record.
pub enum Change {
    Recovered { id: SubmissionId, kind: KindName, to: Status },
    /// The kind is not registered here; the record is `Interrupted`.
    Unclaimed { id: SubmissionId, kind: KindName },
}
```

- Every mutator parses `driver_state.submissions`, applies one
  transition, writes it back, and **returns before any save**. The
  caller owns durability: `store.save` after each successful call, and
  it must not proceed when that save fails, because the transition did
  not become durable. The crate does not depend on a backend.
- `submit` allocates its own id (`next_id`: `<millis:016>-<n:02>`,
  next free in the ledger), so a caller never invents one.
- `retry` covers the two recoverable terminal states. `Interrupted`
  always retries: the process died, nothing else went wrong. `Failed`
  retries only when its `outcome.retryable` is set, otherwise
  `Err(NotRetryable { id })`; the kind's driver decides that at settle
  time, because it is the only one that knows whether the failure was
  its infrastructure or the work. A retried `Failed` record drops its
  `outcome` and keeps its `attempts` count. A `Queued` record needs no
  retry; `start` is its path.
- `reconcile` returns the changes it made so a caller can log or
  surface them, and a test can assert the turn kind's rules in §6 row
  by row. It never writes when nothing changed, so a reconnect does not
  rewrite a large session.

---

## 5. The kind seam

```rust
/// What a crate that owns a kind of work must provide.
pub trait Kind: Send + Sync {
    /// The tag written into `Record::kind`.
    fn name(&self) -> &'static str;

    /// A `Running` record left by a process that is gone. Called once
    /// per stale record at load. Read-only: it may look at the session,
    /// another store, a daemon, but returns a verdict and this crate
    /// does the write.
    fn recover(&self, ctx: &RecoverCtx<'_>) -> Recovery;
}

pub struct RecoverCtx<'a> {
    pub session: &'a Session,
    pub record: &'a Record,
}

impl RecoverCtx<'_> {
    /// The payload, typed by the kind that wrote it.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, SubmissionError>;
}

/// What a dead process left behind. Exactly three answers.
pub enum Recovery {
    /// The work finished; the record is settled.
    Done,
    /// Nothing landed; safe to start again from the payload.
    Queued,
    /// Half done; the driver knows how to continue.
    Interrupted,
}

/// The kinds a frontend links, by tag.
pub struct Kinds { /* BTreeMap<KindName, Arc<dyn Kind>> */ }

impl Kinds {
    pub fn new() -> Self;
    pub fn with(self, kind: Arc<dyn Kind>) -> Self;
    pub fn get(&self, name: &KindName) -> Option<&Arc<dyn Kind>>;
}
```

Rules the seam keeps:

- **Unknown is not fatal.** A record whose kind is not registered
  reconciles to `Interrupted` and is reported as `Change::Unclaimed`.
  A session written by a build that links more extensions still loads,
  and the frontend can tell the user "this record belongs to an
  extension you do not have". A kind is never silently dropped.
- **`recover` is a question, not an action.** It cannot mutate the
  session, cannot save, and runs once per stale record at load. A kind
  that needs to look at a daemon does so read-only; the write stays the
  crate's, so the state machine has one owner.
- **Three verdicts, not more.** `Done`, `Queued`, `Interrupted` are the
  only answers to "what did the crash leave": the work finished,
  nothing landed, or something landed. `Interrupted` already means the
  driver resumes it, and how it resumes is the kind's business.
- **Synchronous.** A frontend doing its recovery before it accepts
  work can afford a read per stale record. A `recover` that must do
  network I/O is a design smell in the kind, not a reason to make this
  async.
- **A kind brings what it needs.** `RecoverCtx` carries only the
  session and the record. A kind that must read another store (a
  subagent loading the child session) holds that store in the struct
  the frontend constructed when it registered the kind, so this crate
  never names a second backend.

The frontend assembles `Kinds` from the extension crates it links. A
harness that does not link subagents never registers that kind and
never sees those records; nothing changes in this crate when it does.

---

## 6. The built-in turn kind

A turn is the one kind this crate ships, because it is the engine's own
unit of work: `Agent::turn`, `Agent::resume_turn`, `Session::history`,
`StopReason`. Everything it needs is `rhema-core`, which this crate
already depends on for `Session`.

It is deliberately **not** in `rhema-coding-agent`: that crate is one
*assembly* of agents (prompt, tools, approval, resolver), and a turn
belongs to any assembly. `rhema-autonomous-agent`, a harness, a
benchmark runner all drive the same `rhema_core::Agent`, so the turn
kind has to be available to all of them. It is not in `rhema-core`
either, because core must not know what a submission is: the dependency
runs this way, not the other.

```rust
pub mod turn {
    use rhema_core::{Agent, EventSink, Session, ToolContext};

    /// A submission of kind `turn`.
    #[derive(Serialize, Deserialize)]
    pub struct Payload {
        /// The prompt, exactly as the caller wrote it.
        pub input: String,
        /// `session.history.len()` when the turn started. The input
        /// lands at this index; the answer is derived after it.
        pub base: Option<u64>,
    }

    pub struct Turn;

    impl Kind for Turn {
        fn name(&self) -> &'static str { "turn" }
        fn recover(&self, ctx: &RecoverCtx<'_>) -> Recovery;
    }

    /// Queue a prompt. One write once the caller saves.
    pub fn submit(session: &mut Session, input: impl Into<String>, at: Millis)
        -> Result<Record, SubmissionError>;

    /// Start, turn, settle, saving at each of the three points.
    pub async fn drive(
        agent: &mut Agent,
        session: &mut Session,
        id: &SubmissionId,
        sink: &mut dyn EventSink,
        ctx: &ToolContext,
        commit: &mut dyn FnMut(&Session) -> Result<(), CommitError>,
    ) -> Result<Record, SubmissionError>;

    /// Continue an interrupted turn: retry, `resume_turn`, settle,
    /// saving at each of the three points.
    pub async fn resume(
        agent: &mut Agent,
        session: &mut Session,
        id: &SubmissionId,
        sink: &mut dyn EventSink,
        ctx: &ToolContext,
        commit: &mut dyn FnMut(&Session) -> Result<(), CommitError>,
    ) -> Result<Record, SubmissionError>;

    /// The answer, from history, for a settled turn.
    pub fn answer(session: &Session, snapshot: &Snapshot, id: &SubmissionId)
        -> Option<String>;
}
```

`drive` is the four steps every frontend would otherwise write, in the
order that makes the crash windows in §7 true:

1. `start(id, Some(Payload { input, base: history.len() }))`, commit;
2. `agent.turn(input, session, sink, ctx)`, whose own commit hook writes
   `Running` plus history at every boundary;
3. map the result to an `Outcome`: `Ok` → `Done`, `Err(e)` →
   `Failed { error: e.to_string(), retryable: e.is_retryable() }`, a
   question core already answers (`AgentError::is_retryable`); `settle`,
   commit.

A dropped `drive` future (the ACP connection went away) leaves the
record `Running`; the caller settles `Cancelled` and commits. That is
the only place a frontend has to know the ordering.

`commit` is the same persistence the agent's `CommitHook` calls. A
frontend wires both to one `store.save`, so the boundary commits and
the three lifecycle writes all land through the same path.

`resume` is the interrupted half: it closes orphaned tool calls with
core's `close_orphaned_calls` (a no-op when the caller already revived
them with the resolver), `retry`s the record and commits, drives
`resume_turn`, settles, commits. A caller that wants replay-safe tool
revival repairs first and then calls `resume`.

`answer` reads `history[base..]` backwards for the last assistant
message's text blocks. It needs no copy in the record, and a session
that predates this crate has no `base` and no answer, which is the
truth.

### Recovery

Ordered rules, first match wins:

| # | History at load | Verdict |
|---|---|---|
| 1 | `base` missing, or `len < base` (a compaction shortened it) | `Interrupted` |
| 2 | `len == base` (the input never landed) | `Queued` |
| 3 | `history[base]` is not the payload's input message | `Interrupted` |
| 4 | the tail is an assistant message with no tool call (`Done` rule) | `Done` |
| 5 | anything else after the input | `Interrupted` |

`base` makes the question exact: no text matching against the prompt, so
two identical prompts in a row cannot confuse it. Rule 4 is the
promotion rule of §7; rules 1, 3 and 5 hand the record to
`turn::resume` above, which repairs dangling calls and drives
`resume_turn` (§9).

### Compaction

`base` is an index into `history`, so a compaction that rewrites history
invalidates it: a running turn reconciles to `Interrupted`, which is
safe, and an old `answer()` returns `None`, which is honest. This spec
states the contract; the compaction spec owns the policy, which is one
of: keep the entries a live record points at, re-base the record's
`base` when history is rewritten, or accept both outcomes.

---

## 7. Crash windows

The generic invariant, true for every kind:

1. The record exists and is durable before any side effect (`submit`).
2. `start` is durable before the work begins.
3. Work writes its own progress in its own store.
4. `settle` happens after the work returns.
5. A `Running` record loaded by a process that did not start it is
   stale, and the kind decides what the world says.

The turn kind's five rules are the table in §6. Two of them carry the
design:

- **The answer's commit is the settle.** The engine's last commit
  carries the final assistant message; a crash right after it leaves a
  `Running` record whose answer is already durable. Recovery promotes
  it to `Done` instead of pretending the turn failed, because re-running
  would bill the model twice for an answer already on disk.
- **The input's commit is the start.** A `Running` record whose input
  is in history must not be replayed with `turn`, which would append
  the same user message twice. `resume_turn` continues the tail.

A kind whose state is elsewhere (a job) has the same five-step
invariant, and its `recover` closes the gap the two stores open: the
daemon may say "exited 0" while the record still says `Running`, and
`recover` returns `Done`. The crate cannot make two stores atomic, and
does not pretend to; it makes the gap explicit and testable per kind.

---

## 8. Ownership

v1 has no leases, and this is the deliberate part most likely to be
questioned:

- One process per session is the store's rule
  (`specs/rhema-session-store.md` §0): no CAS, last writer wins. A
  submission claim cannot be stronger than the store under it, so a
  lease here would be a lock a stale process can walk through.
- A process that *loads* a ledger did not start its `Running` records.
  Nothing in its memory can be driving them, so every `Running` record
  at load is stale by definition. That is why `reconcile` asks no
  question about pids or liveness: the answer is always "not mine".
- `owner.pid` is written for diagnosis only ("who left this running"),
  never read by a decision.

When CAS lands in the store, ownership becomes a real lease: `start`
becomes compare-and-set on the record, the record gains a lease expiry,
and a `Running` record with an unexpired lease is not reconciled. The
owner field is already there for the first half of that change.

---

## 9. The one core addition: `Agent::resume_turn`

`run_turn` appends the input and loops `step` until the model stops
asking for tools. A crashed turn whose input is committed needs the
same loop **without** the append:

```rust
impl Agent {
    /// Drive the step loop over the history's tail, appending nothing.
    /// The answer to a turn whose input is already committed, or the
    /// continuation of one that crashed mid tool loop.
    pub async fn resume_turn(
        &mut self,
        session: &mut Session,
        sink: &mut dyn EventSink,
        ctx: &ToolContext,
    ) -> Result<ModelOutput, AgentError>;
}
```

It is `run_turn` minus its first line, sharing the loop guard and the
commit hook, and it is the only change to `rhema-core` this spec makes.
It mirrors `Agent::turn`: `resume_turn` uses `DEFAULT_TURN_STEPS` and
`resume_turn_with_limit` takes the caller's guard.

Dangling calls are repaired before the model is asked again.
`turn::resume` does the core half (`close_orphaned_calls`, a synthetic
failure for every unanswered call) because that needs no tools. A
driver that wants the richer repair calls `revive_dangling_calls`
(`rhema-agent-tools`) first: it re-executes the replay-safe calls and
needs the resolver, so it stays out of this crate, and `resume` then
finds nothing to close.

---

## 10. Consumers

**ACP (v1).** The one frontend whose disconnect actually loses
information today: a dropped connection cancels the turn, and the next
`session/load` sees a history with no record of what was asked.

- `session/prompt`: `turn::submit` + save, `turn::drive`, and on a
  dropped connection `settle(Cancelled)` + save.
- `session/load`: `reconcile(session, &kinds, now)` + save **only when
  it changed something**, so a plain reconnect does not rewrite a 15 MB
  file. The client then sees an honest file: the last turn is
  `Interrupted` or `Done`, never silently missing.
- No new wire method in v1. An ACP extension to list submissions is a
  wire decision, not a storage one.

**Extension crates.** Each brings its own kind and registers it:

- `rhema-subagents` implements `Kind` with payload
  `{ child: SessionId }`: the child session is saved by the store,
  `recover` loads it and reads the turn submission the subagent started
  there. This is the change `specs/rhema-subagents.md` would need
  (§0 there currently rules persistent subagent sessions out), and it is
  now an implementation of this seam, not a schema change.
- The `job` wiring implements `Kind` with payload
  `{ command, backend_id, tag }`, `recover` reading the daemon's
  listing. It needs one extra move: the session tag (`RHEMA_SESSION`,
  today a per-process `OnceLock` in `rhema-code/src/jobs.rs`) becomes
  session state, or a resumed session cannot see its own jobs.
- A future `ask` kind is a payload and a `recover` that says
  `Interrupted` because a question nobody answered cannot be resumed.

None of this changes this crate.

**TUI and headless: unchanged.** The TUI is single-turn, in-process,
and its `/resume` already repairs a dangling call and shows the last
boundary; a record would add writes and nothing a terminal can reattach
to. Headless is one shot, and `--save` is already its record. Both can
adopt later without a schema change.

**RSD: never.** `driver_state` in an RSD session holds its key/plan
shape; a session is one or the other.

---

## 11. Layout and Cargo

```
crates/rhema-submissions/
  Cargo.toml
  README.md
  src/
    lib.rs        # re-exports; #![forbid(unsafe_code)]
    kind.rs       # Kind, RecoverCtx, Recovery, Kinds
    record.rs     # Record, Status, Outcome, Owner, KindName
    state.rs      # submit/start/settle/cancel/retry/reconcile
    view.rs       # Snapshot
    error.rs      # SubmissionError
    turn.rs       # the built-in Turn kind
  tests/
    lifecycle.rs
    crash_windows.rs
    kinds.rs
```

```toml
[dependencies]
rhema-core = { path = "../rhema-core", version = "0.3.0" }
rhema-session-store = { path = "../rhema-session-store", version = "0.3.0" }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
```

The seam is a type dependency (`SessionId`), not a backend: this crate
compiles with no `rusqlite`, no `rhema-config`, and no frontend. It
depends on neither `rhema-coding-agent` nor `rhema-agent-tools`; the
turn kind needs only core, and the dangling-call repair stays in the
driver that has tools.

`rhema-core` gains `resume_turn` and nothing else.

---

## 12. Tests

Generic lifecycle:

1. **Round trip.** `submit`/`start`/`settle` survive `serde_json`
   through `FileStore::save`/`load` with every field intact, including a
   `driver_state` that already held another key (the key survives) and
   a kind and payload this build does not know.
2. **State machine rejects.** `settle` on `Queued`, `start` on `Done`,
   `cancel` on `Done`: typed errors, records untouched.
3. **Cancel split.** `Queued` → `Settled`; `Running` → `InFlight` and
   no write.
4. **Version and corruption.** `v: 2` is `Unsupported`; an unknown
   status string is `Corrupt`; neither panics, both leave the session
   untouched.
5. **Retention.** 40 settled records become 32, the newest; an
   unsettled record among the 8 dropped ones survives.
6. **Unknown kind.** A `Running` record with `kind: "job"` and no
   registered kind reconciles to `Interrupted`, returns `Unclaimed`,
   and does not touch the payload.
7. **The retry gate.** `retry` on `Failed{retryable: true}` reaches
   `Running` and clears the outcome; on `Failed{retryable: false}` it is
   `NotRetryable` and the record is untouched; on `Interrupted` it
   reaches `Running` with `attempts` incremented.
8. **A registered kind wins.** A test kind whose `recover` returns
   `Done`/`Queued`/`Interrupted` lands each verdict, and its `recover`
   sees its own payload through the typed accessor.
9. **Idempotent reconcile.** Two passes, the second returns no changes,
   including with a registered kind.

Turn kind:

10. **The five rules of §6**, table-driven, each asserting status,
   `answer`, and the driver's next step.
11. **The §6 rules through `drive`**: a session built by `drive` and cut
    at each commit point, then loaded and reconciled, with the next
    step asserted (turn or resume).
12. **`answer` after a shortened history** is `None`.
13. **A turn stopped by the step cap is `Done`.** `StopReason::ToolUse`
    after the last allowed step; the calls ran and the history is valid.
14. **A failure's class.** A provider network error settles
    `Failed { retryable: true }` through `AgentError::is_retryable`; a
    missing provider and a commit error settle `false`.

Core and wire:

15. **`resume_turn`** appends nothing, honors the guard, and fires the
    commit hook at each boundary.
16. **ACP:** a provider that fails after a committed tool call leaves
    `Interrupted` (or `Failed` when the error surfaces); a dropped
    connection leaves `Cancelled`; `session/load` writes once for the
    first reconcile and nothing on the next load.

---

## 13. Resolved during review

1. **Retrying a `Failed` record.** Agreed: retrying is implementation
   dependent, so the kind decides. The record carries the verdict as
   `outcome.retryable`, set by the driver at settle time when it has the
   failure in hand. `retry` then allows `Failed → Running` only for a
   retryable failure, and `Interrupted → Running` unconditionally.
2. **Retention.** 32 **settled submission records** per session, every
   unsettled one kept. This counts records in the ledger, not history
   messages: a record is an id, a tag, a payload, three timestamps and a
   status, so 32 of them are roughly 100 KB with typical payloads.
   Commit to 32 and revisit only with a measurement.

---

## 14. Decisions

- **Records live in `driver_state`, not a table.** With the record
  inside the session, one file carries the work's state and the status
  for every kind whose state is the session, and recovery reads both
  from one atomic write on every backend. A table makes them two
  commits in two places, and closing that window needs a transaction
  across two records, which the file backend cannot express.
- **The kind is a validated string plus a trait, not an enum.** The
  lifecycle is universal; the payload and the meaning of "finished" are
  not. A closed set would make every future consumer an edit to this
  crate.
- **Three recovery verdicts.** `Done`, `Queued`, `Interrupted` are the
  only answers to "what did the crash leave", and every kind's recovery
  is a function into them.
- **Unknown kinds reconcile to `Interrupted` and are reported.** A
  session never refuses to load because an extension is missing, a
  record is never silently dropped, and `reconcile` returns
  `Change::Unclaimed` so a frontend can log it. The wire grows a field
  when an extension that ships kinds needs one.
- **The crate saves nothing.** It mutates the session and returns;
  durability stays the caller's `store.save`, one call per transition.
- **`SubmissionId` is `SessionId`.** One alphabet, one validator, and
  an id already usable as a child session name.
- **No result copy.** The record says done; the result is the kind's to
  derive from its own store.
- **The built-in kind is `turn`, in this crate.** A turn is core's unit
  of work; coding-agent is an assembly, not an owner.
- **No `Waiting` status.** A status says who owns the record: nothing
  (`Queued`), a live owner (`Running`), or the end. A kind that blocks
  on a human or a port is still `Running`; the payload carries the
  phase, and a status is not a UI label.
- **A retry of `Failed` is the kind's call, recorded once.**
  `outcome.retryable` is written by the driver at settle time, when the
  failure class is known, not guessed later from an error string.
- **32 settled records.** A fixed cap, not configuration: the ledger
  must not grow with the session's life, and 32 covers what a picker or
  a reconnect ever shows.
- **No leases in v1.** See §8.
- **`reconcile` never consults pids.** A ledger loaded from disk has no
  live owner by construction.

---

## 15. Acceptance

- `cargo test -p rhema-submissions`, `-p rhema-core`, and `-p rhema-acp`
  pass; `cargo clippy --all-targets -- -D warnings` is clean.
- The five turn recovery rules and the cut-at-each-commit cases each
  have a test that asserts status, answer, and the next step.
- A session holding a record of an unregistered kind loads, lists,
  reconciles to `Interrupted`, and reports `Unclaimed`.
- An ACP turn killed after a boundary leaves a session that
  `session/load` reconciles to `Interrupted`, and a surviving answer to
  `Done`.
- A session written before this crate, with `driver_state: null`, loads,
  lists, and resumes unchanged; no empty ledger is stamped on it.
- `cargo tree -p rhema-submissions | rg "rhema-(coding-agent|agent-tools)"`
  finds no match: the turn kind needs core only.
