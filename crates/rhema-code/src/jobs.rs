//! The status strip's view of the session's background jobs.
//!
//! The jobs themselves are the agent shell's: `job run` starts one through
//! the backend, `job ls` lists this session's. The frontend's job is only
//! to show them while they run, so this module polls
//! `rhema __jobs ls --json --session <id>` on a slow tick and hands the
//! last answer to the renderer. Nothing here parses the backend's output
//! or knows its name: the shim is the whole interface.
//!
//! A poll per tick, not a subscription: the backend has no event stream,
//! and a listing is one short process. The tick is fast while something
//! is running and slow when nothing is, so an idle session costs one
//! process every few seconds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhema_agent_tools::jobs::{EXIT_NO_BACKEND, JobRow};

/// How often the listing is re-read while a job is live.
const LIVE_TICK: Duration = Duration::from_millis(1200);

/// How often it is re-read with no jobs at all: a new one can only appear
/// when the model starts it, and that arrives with a turn of its own.
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

/// The session's job view: a shared snapshot the poll thread replaces and
/// the renderer reads without touching the filesystem or a process.
pub struct JobsHandle {
    entries: Arc<Mutex<Vec<JobEntry>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for JobsHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl JobsHandle {
    /// Starts the poll thread, or `None` when there is nothing to poll:
    /// no backend installed, or no binary to run the shim from.
    pub fn spawn() -> Option<Self> {
        if !rhema_agent_tools::jobs::available() {
            return None;
        }
        let bin = std::env::current_exe().ok()?;
        let session = session_id().to_owned();
        let entries = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_entries = Arc::clone(&entries);
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("rhema-jobs".to_owned())
            .spawn(move || poll(bin, session, thread_entries, thread_stop))
            .ok()?;
        Some(Self { entries, stop })
    }

    /// The snapshot the last poll left, oldest first.
    pub fn entries(&self) -> Vec<JobEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }
}

/// This process's session id: what every job it starts is tagged with,
/// and what the strip filters on. Unique per process, short enough to
/// stay inside a description tag, and never shown to the model: the shim
/// writes it, the model never types it.
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
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// The poll loop: sleep, ask, replace. Exits when the handle drops or the
/// backend vanishes.
fn poll(
    bin: std::path::PathBuf,
    session: String,
    shared: Arc<Mutex<Vec<JobEntry>>>,
    stop: Arc<AtomicBool>,
) {
    let mut first_seen: HashMap<String, Instant> = HashMap::new();
    let mut tick = IDLE_TICK;
    loop {
        // Sleep in slices so a drop stops the thread promptly rather than
        // after a whole idle tick.
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
            // The backend is gone: there is nothing left to observe, and
            // the next poll would fail the same way.
            return;
        }
        let rows: Vec<JobRow> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        let now = Instant::now();
        first_seen.retain(|id, _| rows.iter().any(|row| &row.id == id));
        let mut entries: Vec<JobEntry> = rows
            .into_iter()
            .filter(|row| row.status.starts_with("running"))
            .map(|row| {
                let since = *first_seen.entry(row.id.clone()).or_insert(now);
                JobEntry {
                    row,
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
