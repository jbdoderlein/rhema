//! The session's view of its background jobs: the strip's running
//! lines, and the finish feed that announces an end to the model.
//!
//! The jobs themselves are the agent shell's: `job run` starts one
//! through the backend, `job ls` lists this session's. The frontend
//! polls `rhema __jobs ls --json --session <id>` on a slow tick and
//! splits each answer in two. Running rows feed the status strip. A
//! row that was running at the last poll and is not running now is a
//! finish, minted once into a feed with two readers: the transcript's
//! notice, and the driver's steering, the subagent registry's
//! delivery shape over a listing instead of a registry, so a job the
//! model started reaches it without a poll of its own.
//!
//! A poll per tick, not a subscription: the backend does stream
//! events (`gob events`), but the shim's listing is the whole
//! interface the frontend reads, and a subscription would tie the
//! frontend to the backend's name and line format. The poll keeps the
//! backend swappable at one short process per tick, fast while
//! something is running, slow when nothing is. Detection latency is
//! one live tick; delivery, once detected, is the registry's and
//! instant.
//!
//! One delivery per finish, like a subagent's: steering mid-turn or
//! the idle prompt, never both. The registry earns that with its
//! fetch, the model fetching the report retires everything, so
//! here the delivery itself retires: the drain consumes the line,
//! because the exit code was the whole message. The divergence is
//! the point where the mirror ends, and it is the registry's own
//! rule telling the truth: a query-only drain protects a fetch, and
//! a job has none.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhema_agent_tools::jobs::{EXIT_NO_BACKEND, JobRow};
use rhema_coding_agent::Steering;

/// How often the listing is re-read while a job is live.
const LIVE_TICK: Duration = Duration::from_millis(1200);

/// How often it is re-read with no jobs at all: a new one can only
/// appear when the model starts it, and that arrives with a turn of
/// its own.
const IDLE_TICK: Duration = Duration::from_secs(5);

/// One job as the strip draws it: the shim's row plus the age this
/// process has watched it, which is the only clock available without
/// parsing the backend's timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEntry {
    /// What the shim reported.
    pub row: JobRow,
    /// Time since this process first saw the job.
    pub age: Duration,
}

/// One finished job, as the finish feed delivers it: the transcript
/// notice a frontend pushes the moment a job ends, and, in `line`,
/// the same announcement the driver steers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFinished {
    /// The backend's job id: what `job logs` and `job stop` take.
    pub id: String,
    /// The shell line as the model wrote it.
    pub command: String,
    /// The process's exit code, when the backend reported one. `None`
    /// means a stop that came with no code, a signal, or a job gone
    /// from the listing under us.
    pub exit_code: Option<i32>,
    /// The notification line: the notice's text and the steering's.
    pub line: String,
}

/// One finish between minting and its two takes. The latches mirror
/// the subagent registry's: `noticed` retires the transcript notice,
/// `announced` retires the notification line here and in the steering
/// drain. A finish survives until both land, so a notice never costs
/// the model its steering line, and a taken prompt never repeats.
struct Pending {
    finished: JobFinished,
    noticed: bool,
    announced: bool,
}

/// The finish feed: minted transitions, oldest first. The poll thread
/// mints, the frontend takes, and the driver reads through
/// [`JobsHandle::steering`].
#[derive(Default)]
struct Feed {
    pending: Vec<Pending>,
}

impl Feed {
    /// Mints one finish: the row as it was last seen running, plus the
    /// exit code the listing reports now, `None` when it reports none
    /// or the job vanished.
    fn mint(&mut self, seen: JobRow, exit_code: Option<i32>) {
        let line = announcement(&seen.id, &seen.command, exit_code);
        self.pending.push(Pending {
            finished: JobFinished {
                id: seen.id,
                command: seen.command,
                exit_code,
                line,
            },
            noticed: false,
            announced: false,
        });
    }

    /// The pending notification lines, oldest first, a pure query:
    /// the same lines come back until a take retires them.
    fn lines(&self) -> Vec<String> {
        self.pending
            .iter()
            .filter(|pending| !pending.announced)
            .map(|pending| pending.finished.line.clone())
            .collect()
    }
}

/// The notification line for one finished job, the driver's steering
/// vocabulary and the finish notice's text, in the registry's shape:
/// what ended, and how.
fn announcement(id: &str, command: &str, exit_code: Option<i32>) -> String {
    match exit_code {
        Some(0) => format!("Background job {id} (\"{command}\") has finished (exit 0)."),
        Some(code) => {
            format!("Background job {id} (\"{command}\") has finished with an error (exit {code}).")
        }
        None => format!("Background job {id} (\"{command}\") was stopped."),
    }
}

/// The transition pass, run once per listing: every job last seen
/// running that the listing no longer reports as running has finished.
/// A stopped row gives the exit code; a vanished row (removed under
/// us) finishes with none. `running` keeps the first-seen order, so
/// finishes mint oldest first.
fn observe(running: &mut Vec<(String, JobRow)>, feed: &mut Feed, rows: &[JobRow]) {
    let mut alive: Vec<(String, JobRow)> = Vec::new();
    for (id, seen) in running.drain(..) {
        match rows.iter().find(|row| row.id == id) {
            Some(row) if row.status.starts_with("running") => alive.push((id, row.clone())),
            Some(row) => feed.mint(seen, row.exit_code),
            None => feed.mint(seen, None),
        }
    }
    for row in rows.iter().filter(|row| row.status.starts_with("running")) {
        if !alive.iter().any(|(id, _)| *id == row.id) {
            alive.push((row.id.clone(), row.clone()));
        }
    }
    *running = alive;
}

/// The session's job view: a shared snapshot the poll thread replaces
/// and the renderer reads without touching the filesystem or a
/// process, plus the finish feed the delivery paths read.
pub struct JobsHandle {
    entries: Arc<Mutex<Vec<JobEntry>>>,
    feed: Arc<Mutex<Feed>>,
    stop: Arc<AtomicBool>,
}

impl Drop for JobsHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl JobsHandle {
    /// Starts the poll thread, or `None` when there is nothing to
    /// poll: no backend installed, or no binary to run the shim from.
    pub fn spawn() -> Option<Self> {
        if !rhema_agent_tools::jobs::available() {
            return None;
        }
        let bin = std::env::current_exe().ok()?;
        let session = session_id().to_owned();
        let entries = Arc::new(Mutex::new(Vec::new()));
        let feed = Arc::new(Mutex::new(Feed::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_entries = Arc::clone(&entries);
        let thread_feed = Arc::clone(&feed);
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("rhema-jobs".to_owned())
            .spawn(move || poll(bin, session, thread_entries, thread_feed, thread_stop))
            .ok()?;
        Some(Self {
            entries,
            feed,
            stop,
        })
    }

    /// The snapshot the last poll left, oldest first.
    pub fn entries(&self) -> Vec<JobEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    /// Takes the finish events out, oldest first, one per job,
    /// exactly once. The transcript notice feed: a frontend pushes
    /// each event's `line` the moment the job ends, whatever turn is
    /// running. Taking a notice retires nothing else: the line stays
    /// pending for the driver's steering until
    /// [`Self::take_notifications`].
    pub fn take_finished(&self) -> Vec<JobFinished> {
        let Ok(mut feed) = self.feed.lock() else {
            return Vec::new();
        };
        let taken: Vec<JobFinished> = feed
            .pending
            .iter()
            .filter(|pending| !pending.noticed)
            .map(|pending| pending.finished.clone())
            .collect();
        for pending in feed.pending.iter_mut() {
            pending.noticed = true;
        }
        feed.pending
            .retain(|pending| !(pending.noticed && pending.announced));
        taken
    }

    /// Takes the pending notification lines out, oldest first,
    /// retiring them here and in the steering drain. The idle path: a
    /// frontend with no running turn runs the lines as the next
    /// turn's prompt, and this take keeps the driver's turn-start
    /// drain from repeating what the prompt already said. While a
    /// turn runs, the lines belong to the driver's steering: do not
    /// call this.
    pub fn take_notifications(&self) -> Vec<String> {
        let Ok(mut feed) = self.feed.lock() else {
            return Vec::new();
        };
        let lines = feed.lines();
        for pending in feed.pending.iter_mut() {
            pending.announced = true;
        }
        feed.pending
            .retain(|pending| !(pending.noticed && pending.announced));
        lines
    }

    /// The driver's steering source over this feed: one notification
    /// line per finished-but-undelivered job, and the drain consumes,
    /// a steered line never comes back, so a finish the driver
    /// announced mid-turn does not re-deliver as the idle session's
    /// prompt. A frontend with both feeds chains this with the
    /// subagent registry's through `SteeringChain` into the config's
    /// one `steering` slot; the registry's drain stays a query, its
    /// retirement being the fetch or the idle take.
    pub fn steering(&self) -> Arc<dyn Steering> {
        Arc::new(JobSteering(Arc::clone(&self.feed)))
    }
}

/// The steering source over one feed. Unlike the subagent registry's,
/// the drain retires what it returns: a steered line is delivered,
/// its whole payload, the exit code, rode inside it, and the card
/// entered the transcript when the finish was detected. The registry
/// re-announces because an unfetched report is expensive work that
/// must not be lost; a job has nothing to fetch.
struct JobSteering(Arc<Mutex<Feed>>);

impl Steering for JobSteering {
    fn drain(&self) -> Vec<String> {
        let Ok(mut feed) = self.0.lock() else {
            return Vec::new();
        };
        let lines = feed.lines();
        for pending in feed.pending.iter_mut() {
            pending.announced = true;
        }
        // A finish both noticed and announced is fully delivered. One
        // announced but never noticed (headless: no transcript exists
        // to notice it) stays in the feed, inert, until the handle
        // drops, bounded by the job count, never re-delivered.
        feed.pending
            .retain(|pending| !(pending.noticed && pending.announced));
        lines
    }
}

/// This process's session id: what every job it starts is tagged with,
/// and what the strip filters on. Unique per process, short enough to
/// stay inside a description tag, and never shown to the model: the
/// shim writes it, the model never types it.
pub fn session_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // `s` for readability in the backend's own listing, then the pid
        // and the low nanos: a restarted process lands on a different id
        // even where the pid was reused.
        format!("s{:x}{:06x}", std::process::id(), nanos & 0xff_ffff)
    })
}

#[cfg(test)]
impl JobsHandle {
    /// A handle over a fixed snapshot: the renderer's tests draw the
    /// strip without a thread, a backend, or a clock.
    pub(crate) fn stub(entries: Vec<JobEntry>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(entries)),
            feed: Arc::new(Mutex::new(Feed::default())),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A handle over a pre-minted finish feed: the lifecycle's tests
    /// deliver notifications without a thread, a backend, or a poll.
    pub(crate) fn stub_with_finished(finished: Vec<JobFinished>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            feed: Arc::new(Mutex::new(Feed {
                pending: finished
                    .into_iter()
                    .map(|finished| Pending {
                        finished,
                        noticed: false,
                        announced: false,
                    })
                    .collect(),
            })),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// The poll loop: sleep, ask, split. Exits when the handle drops or
/// the backend vanishes.
fn poll(
    bin: std::path::PathBuf,
    session: String,
    shared: Arc<Mutex<Vec<JobEntry>>>,
    feed: Arc<Mutex<Feed>>,
    stop: Arc<AtomicBool>,
) {
    let mut running: Vec<(String, JobRow)> = Vec::new();
    let mut first_seen: HashMap<String, Instant> = HashMap::new();
    let mut tick = IDLE_TICK;
    loop {
        // Sleep in slices so a drop stops the thread promptly rather
        // than after a whole idle tick.
        let mut slept = Duration::ZERO;
        while slept < tick {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
            slept += Duration::from_millis(100);
        }
        let output = std::process::Command::new(&bin)
            .arg(rhema_agent_tools::jobs::MODE)
            .arg("ls")
            .arg("--json")
            .arg("--session")
            .arg(&session)
            .output();
        let Ok(output) = output else { continue };
        if output.status.code() == Some(EXIT_NO_BACKEND) {
            // The backend is gone: there is nothing left to observe,
            // and the next poll would fail the same way.
            return;
        }
        // A listing that fails to parse is not an empty one: reading
        // it as empty would mint a vanish-finish for every running
        // job. Skip the tick and let the next listing answer.
        let Ok(rows) = serde_json::from_slice::<Vec<JobRow>>(&output.stdout) else {
            continue;
        };
        if let Ok(mut feed) = feed.lock() {
            observe(&mut running, &mut feed, &rows);
        }
        let now = Instant::now();
        first_seen.retain(|id, _| rows.iter().any(|row| &row.id == id));
        let mut entries: Vec<JobEntry> = rows
            .iter()
            .filter(|row| row.status.starts_with("running"))
            .map(|row| {
                let since = *first_seen.entry(row.id.clone()).or_insert(now);
                JobEntry {
                    row: row.clone(),
                    age: now.saturating_duration_since(since),
                }
            })
            .collect();
        entries.sort_by_key(|entry| entry.age);
        tick = if entries.is_empty() {
            IDLE_TICK
        } else {
            LIVE_TICK
        };
        if let Ok(mut guard) = shared.lock() {
            *guard = entries;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row as the listings carry it.
    fn row(id: &str, status: &str, exit_code: Option<i32>) -> JobRow {
        JobRow {
            id: id.to_owned(),
            status: status.to_owned(),
            exit_code,
            command: "npm run dev".to_owned(),
            session: Some("s1".to_owned()),
            started_at: None,
        }
    }

    #[test]
    fn a_running_job_that_stops_mints_one_finish() {
        let mut running = vec![("abc".to_owned(), row("abc", "running", None))];
        let mut feed = Feed::default();
        observe(&mut running, &mut feed, &[row("abc", "stopped", Some(1))]);
        assert!(running.is_empty(), "the job left the running set");
        assert_eq!(feed.pending.len(), 1);
        let finished = &feed.pending[0].finished;
        assert_eq!(finished.id, "abc");
        assert_eq!(finished.exit_code, Some(1));
        assert_eq!(
            finished.line,
            "Background job abc (\"npm run dev\") has finished with an error (exit 1)."
        );
    }

    #[test]
    fn a_stopped_job_never_seen_running_mints_nothing() {
        let mut running = Vec::new();
        let mut feed = Feed::default();
        observe(&mut running, &mut feed, &[row("abc", "stopped", Some(0))]);
        assert!(feed.pending.is_empty(), "no watch, no finish");
        // A still-running job is taken up instead.
        observe(&mut running, &mut feed, &[row("abc", "running", None)]);
        assert!(feed.pending.is_empty());
        assert_eq!(running.len(), 1);
    }

    #[test]
    fn a_job_that_vanishes_from_the_listing_finishes_stopped() {
        let mut running = vec![("abc".to_owned(), row("abc", "running", None))];
        let mut feed = Feed::default();
        observe(&mut running, &mut feed, &[]);
        assert_eq!(feed.pending.len(), 1);
        assert_eq!(feed.pending[0].finished.exit_code, None);
        assert_eq!(
            feed.pending[0].finished.line,
            "Background job abc (\"npm run dev\") was stopped."
        );
    }

    #[test]
    fn finishes_mint_oldest_first() {
        let mut running = vec![
            ("aaa".to_owned(), row("aaa", "running", None)),
            ("bbb".to_owned(), row("bbb", "running", None)),
        ];
        let mut feed = Feed::default();
        observe(
            &mut running,
            &mut feed,
            &[
                row("aaa", "stopped", Some(0)),
                row("bbb", "stopped", Some(2)),
            ],
        );
        let ids: Vec<&str> = feed
            .pending
            .iter()
            .map(|pending| pending.finished.id.as_str())
            .collect();
        assert_eq!(ids, vec!["aaa", "bbb"]);
    }

    #[test]
    fn the_exit_zero_vocabulary_says_finished() {
        assert_eq!(
            announcement("abc", "npm run dev", Some(0)),
            "Background job abc (\"npm run dev\") has finished (exit 0)."
        );
        assert_eq!(
            announcement("abc", "npm run dev", None),
            "Background job abc (\"npm run dev\") was stopped."
        );
    }

    #[test]
    fn the_steering_drain_delivers_exactly_once() {
        let handle = JobsHandle::stub_with_finished(vec![JobFinished {
            id: "abc".to_owned(),
            command: "npm run dev".to_owned(),
            exit_code: Some(0),
            line: "Background job abc (\"npm run dev\") has finished (exit 0).".to_owned(),
        }]);
        let line = "Background job abc (\"npm run dev\") has finished (exit 0).".to_owned();
        // The notice: once.
        assert_eq!(handle.take_finished().len(), 1);
        assert!(handle.take_finished().is_empty());
        // The steering delivers the line, and consumes it. A steered
        // finish never re-delivers as the idle prompt: the exit code
        // rode inside the line, there is nothing left to fetch.
        assert_eq!(handle.steering().drain(), vec![line.clone()]);
        assert!(handle.steering().drain().is_empty());
        assert!(handle.take_notifications().is_empty());
    }
}
