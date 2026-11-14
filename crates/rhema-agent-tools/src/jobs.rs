//! The `job` macro: background shells the model starts now and reads
//! from on a later turn.
//!
//! A dev server, a watcher, or a build outlives one `bash` call, and a
//! foreground call that waits on it burns the tool's timeout and the
//! model's context. So the surface is a bash function, the same shape as
//! [`crate::vision`]:
//!
//! ```ignore
//! job run "npm run dev"
//! job logs abc
//! job stop abc
//! ```
//!
//! The process supervision is not ours: a backend CLI owns the daemon,
//! the logs, the exit codes, and the readiness heuristics. Today that is
//! `gob`, and [`BACKEND`] is the only place its name appears. This module
//! adds the three things the backend cannot know:
//!
//! - **A shell string.** `gob add` takes an argv, so a model writing
//!   `gob add 'echo hi > f'` gets `echo` with those as arguments and a
//!   clean exit code. [`run_command`] wraps the model's line in
//!   `bash -c`, so shell syntax keeps meaning what it looks like.
//! - **Attribution.** Every job this session starts carries a
//!   `[<session>]` prefix in its description, and [`ls_command`] filters
//!   on it: two sessions in one workspace see their own jobs, `--dir` is
//!   the deliberate shared view of one directory, and `--all` is the
//!   machine's.
//! - **The self-stop record.** The verbs that end a job (`stop`, `rm`,
//!   `shutdown`) append the ids they stopped to a session-scoped file
//!   under the temp dir, and the frontend's poller reads it
//!   ([`stopped_ids`]) before minting a finish: a job the model ordered
//!   stopped must not come back as a notice telling it what it just
//!   did. The record is keyed by the session tag, so a stop aimed at
//!   another session's job never silences that session's own feed.
//! - **Refusal.** The backend's own dedupe reuses an identical running
//!   command and rewrites its description, which would hand one session's
//!   server to another. [`run_command`] lists first and refuses instead,
//!   naming the owner.
//!
//! The prompt promises the model a notification when a job finishes or
//! dies. The shim itself stays forwarding-only: the notification is a
//! session-side delivery, made true by the frontend's poller over this
//! same listing, never a backend feature and never a shim verb.
//!
//! Everything else is a forwarding shim, and stays one so the backend can
//! be replaced without the prompt, the shim name, or the frontend moving.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

/// The bash function's name, and the model's whole interface. A rename is
/// this constant, the prompt section, and the spec text.
pub const NAME: &str = "job";

/// The hidden binary mode the init file's function execs: `rhema
/// __jobs …`. The public interface is the function; the mode is a
/// transport detail, like the MCP CLI's `__server`.
pub const MODE: &str = "__jobs";

/// The env var non-interactive bash reads its init file from. Shared with
/// the vision and MCP provisions, so whichever file `BASH_ENV` names
/// last, its baked predecessor line reaches the others.
pub const VAR_BASH_ENV: &str = "BASH_ENV";

/// The env var holding the binary the function execs.
pub const VAR_BIN: &str = "RHEMA_JOBS_BIN";

/// The env var carrying the session id the jobs are tagged with.
pub const VAR_SESSION: &str = "RHEMA_SESSION";

/// The backend CLI this shim drives. Swapping it means a new name, the
/// argv translations in one module, and nothing else: the prompt, the
/// function name, and the frontend read this shim, never the backend.
pub const BACKEND: &str = "gob";

/// The longest owner tag [`owner_of`] trusts: the backend's ids are 3
/// characters today, and a longer token means the description's brackets
/// are something else entirely.
const MAX_ID_CHARS: usize = 16;

/// Exit code for a shell the wiring never reached: no session var, so no
/// tag to write and no owner to check.
pub const EXIT_NOT_WIRED: i32 = 4;

/// Exit code when the backend binary is missing from the machine. The
/// frontend's poller reads it as "no jobs here, stop asking".
pub const EXIT_NO_BACKEND: i32 = 5;

/// The system-prompt section a wired frontend renders: the macro's whole
/// documentation, as [`include_str!`] of the file beside this module,
/// the same crate-root data file the system template ships as. No
/// trailing newline: the template interpolates the bytes verbatim.
pub const PROMPT_SECTION: &str = include_str!("../SYSTEM_JOBS.md");

/// The usage text `job` prints when asked, and when given a verb it does
/// not know.
const USAGE: &str = "job: background shells for this session

    job run <command>          start a shell line in the background
    job ls [--dir] [--all]     this session's jobs (--dir: this directory's,
                               --all: the machine's)
    job logs <id>              a job's output so far
    job logs                  every job's, here
    job stop <id>              stop it
    job wait <id>              block until it finishes
    job rm <id>                forget a stopped job
    job shutdown               stop every job this session started";

/// One process's wiring: the env map a frontend layers over
/// `CodingAgentConfig::shell_env`, the prompt section to render, and the
/// provisioned init file held for the process's life.
pub struct Wiring {
    /// The env map: `BASH_ENV`, `RHEMA_JOBS_BIN`, and `RHEMA_SESSION`.
    /// Deliberately no inherited-`BASH_ENV` entry: the chain is baked
    /// into the init file's text, and an env var here would make an outer
    /// generated init re-source itself until the shell died.
    pub shell_env: BTreeMap<String, String>,
    /// The prompt's `jobs` section text, verbatim.
    pub section: &'static str,
    /// The init file's path: what the next provisioner (the vision wire,
    /// then MCP's) bakes into its own init as the literal predecessor, so
    /// every generated file loads in one chain.
    pub init_path: PathBuf,
    dir: PathBuf,
}

impl Drop for Wiring {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static WIRED: OnceLock<Option<Arc<Wiring>>> = OnceLock::new();

/// The process's wiring, built on first use and kept for the process's
/// life. `None` when the backend is not installed: a machine without
/// `gob` gets no `job` function and no prompt section, rather than a
/// command that fails at the model's first use.
///
/// The production seam: the binary exec'd is `current_exe()`. Tests drive
/// [`provision_with_bin`] directly instead, because the process-wide
/// cache makes [`wire`] unusable inside a shared test process.
pub fn wire(bash_env: Option<OsString>, session: &str) -> Option<&'static Arc<Wiring>> {
    WIRED
        .get_or_init(|| {
            if !available() {
                return None;
            }
            let bin = std::env::current_exe().ok()?;
            provision_with_bin(bash_env, &bin, session)
                .map(Arc::new)
                .ok()
        })
        .as_ref()
}

/// What the one edition of the backend on `PATH` can do.
enum Installed {
    /// No backend: a session without `job`, which is not a fault.
    Absent,
    /// A backend with the flag this module cannot work without.
    Ready,
    /// Installed, answers `--version`, and cannot tag a job.
    TooOld(&'static str),
}

/// What `PATH` holds, probed once per process.
///
/// The probe asks for the flag rather than the version number: `gob add
/// --description` is the whole attribution mechanism, and the backend
/// reports its version as `dev` when built from source, so a number would
/// gate nothing. A backend that lacks the flag would take the job and
/// lose its owner, which is worse than refusing to wire at all.
fn installed() -> &'static Installed {
    static STATE: OnceLock<Installed> = OnceLock::new();
    STATE.get_or_init(|| probe(Path::new(BACKEND)))
}

/// The probe itself, on a named binary: the cached [`installed`] asks
/// `PATH`, the tests ask a script written for the question.
fn probe(backend: &Path) -> Installed {
    // `help add`, never `add --help`: the backend forwards unknown flags
    // after the verb to the job it is starting, so `add --help` tries to
    // run a program called `--help`.
    let Ok(help) = Command::new(backend).arg("help").arg("add").output() else {
        return Installed::Absent;
    };
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    if help.status.success() && text.contains("--description") {
        return Installed::Ready;
    }
    Installed::TooOld(Box::leak(
        format!(
            "`{BACKEND}` is too old: `{NAME}` needs `{BACKEND} add --description` (gob 3.x). Install the release tarball or the Homebrew tap; `go install ...@latest` fetches 1.x, because the module path has no major-version suffix."
        )
        .into_boxed_str(),
    ))
}

/// Whether the backend CLI on `PATH` can serve this module, probed once
/// per process. This is what keeps a missing backend quiet everywhere
/// else: no function, no prompt section, no strip.
pub fn available() -> bool {
    matches!(installed(), Installed::Ready)
}

/// The one-line reason `job` is switched off, when the backend is
/// installed but cannot do the job. `None` when the backend is fine and
/// when it is absent: nobody needs a warning for a feature they never
/// asked to install.
pub fn backend_problem() -> Option<&'static str> {
    match installed() {
        Installed::TooOld(reason) => Some(reason),
        Installed::Absent | Installed::Ready => None,
    }
}

/// Provisions one process: the 0700 dir, the init file, the env map.
/// `bin` is the binary the function execs (production passes
/// `current_exe()`, tests pass whichever target speaks [`MODE`]).
///
/// # Errors
///
/// Filesystem failures: the dir cannot be created or the init file cannot
/// be written.
pub fn provision_with_bin(
    bash_env: Option<OsString>,
    bin: &Path,
    session: &str,
) -> std::io::Result<Wiring> {
    let dir = std::env::temp_dir().join(format!(
        "rhema-jobs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let init_path = dir.join("init.bash");
    let mut text = String::from("# generated by rhema-agent-tools; do not edit\n");
    // The predecessor, first and guarded. The path is baked into the file,
    // never carried by an env var: the executor layers the map onto every
    // command, so a var would reach every generated init in a chain and an
    // outer one would re-source itself until bash died. A literal path
    // sources exactly once.
    if let Some(inherited) = bash_env {
        let quoted = inherited.to_string_lossy().replace('\'', "'\\''");
        text.push_str(&format!("source '{quoted}' || true\n"));
    }
    text.push_str(&format!("{NAME}() {{ \"${VAR_BIN}\" {MODE} \"$@\"; }}\n"));
    std::fs::write(&init_path, text)?;

    let mut shell_env = BTreeMap::new();
    shell_env.insert(VAR_BASH_ENV.to_owned(), init_path.display().to_string());
    shell_env.insert(VAR_BIN.to_owned(), bin.display().to_string());
    shell_env.insert(VAR_SESSION.to_owned(), session.to_owned());

    Ok(Wiring {
        shell_env,
        section: PROMPT_SECTION,
        init_path,
        dir,
    })
}

/// One job as the shim reports it: the backend's fields the frontend
/// draws, with the command unwrapped from the `bash -c` this module
/// added. The frontend parses `job ls --json` into these and never talks
/// to the backend itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    /// The backend's job id: what `logs`, `stop`, and `wait` take.
    pub id: String,
    /// `running`, `stopped`, or whatever the backend names it.
    pub status: String,
    /// The process's exit code once stopped, when the backend reports one.
    pub exit_code: Option<i32>,
    /// The shell line as the model wrote it, unwrapped.
    pub command: String,
    /// The session that started it, from the description tag. `None` for
    /// a job started outside this shim.
    pub session: Option<String>,
    /// When the backend says the job started (its own clock and format).
    pub started_at: Option<String>,
}

/// The backend's listing entry: only the fields this module reads, with
/// every optional one defaulted, so a backend that adds fields keeps
/// working.
#[derive(Debug, Clone, Deserialize)]
struct BackendJob {
    id: String,
    status: String,
    command: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    workdir: Option<String>,
}

impl BackendJob {
    /// The row the shim prints and the frontend parses.
    fn row(&self) -> JobRow {
        JobRow {
            id: self.id.clone(),
            status: self.status.clone(),
            exit_code: self.exit_code,
            command: unwrap_command(&self.command),
            session: owner_of(self.description.as_deref()),
            started_at: self.started_at.clone(),
        }
    }
}

/// A backend command line as the model wrote it: the `bash -c` wrapper
/// this module adds is undone, and a job started outside the shim shows
/// its argv joined by spaces.
fn unwrap_command(command: &[String]) -> String {
    match command {
        [shell, flag, line] if shell == "bash" && flag == "-c" => line.clone(),
        _ => command.join(" "),
    }
}

/// The session id a description is tagged with, if any: `[<id>] <cmd>`.
fn owner_of(description: Option<&str>) -> Option<String> {
    let rest = description?.strip_prefix('[')?;
    let (id, _) = rest.split_once(']')?;
    let id = id.trim();
    (!id.is_empty() && id.chars().count() <= MAX_ID_CHARS).then(|| id.to_owned())
}

/// The description a job this session starts carries.
fn tagged(session: &str, command: &str) -> String {
    format!("[{session}] {command}")
}

/// The record of the jobs a session stopped through this shim: one id
/// per line, in the temp dir, keyed by the session tag. The ending
/// verbs append; the frontend's poller reads before minting. The file
/// is append-only and never pruned: a record without its stop
/// suppresses a mint for an id that never mints anyway, and a session's
/// ids are short, so the file stays a list of a few tokens for the
/// session's life.
fn stopped_record_path(session: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rhema-jobs-{session}-stopped"))
}

/// Appends `ids` to the record at `path`, one write: a short
/// append-mode write lands whole or not at all, so a concurrent reader
/// never sees a torn line.
fn record_stopped_in(path: &Path, ids: &[String]) -> std::io::Result<()> {
    use std::io::Write as _;
    if ids.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut text = String::new();
    for id in ids {
        text.push_str(id.trim());
        text.push('\n');
    }
    file.write_all(text.as_bytes())
}

/// The ids recorded at `path`. An absent file records nothing; a line
/// torn by a failing write simply never matches an id, and an id that
/// never matches mints as before.
fn stopped_ids_in(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|text| text.lines().map(str::trim).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Removes the record at `path`: the session is over, its suppressions
/// are history. Best effort.
fn forget_stopped_in(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Records the ids this session's verbs stopped, so the finish feed
/// ([`stopped_ids`]) announces nothing for them: the model ordered
/// those ends, its own commands were the confirmation. No session, no
/// record.
pub fn record_stopped(session: Option<&str>, ids: &[String]) {
    let Some(session) = session.filter(|session| !session.is_empty()) else {
        return;
    };
    let _ = record_stopped_in(&stopped_record_path(session), ids);
}

/// The ids this session stopped through the shim: what the frontend's
/// poller consults before minting a finish.
pub fn stopped_ids(session: &str) -> HashSet<String> {
    stopped_ids_in(&stopped_record_path(session))
}

/// Forgets the record: the session's process is on its way out.
pub fn forget_stopped(session: &str) {
    forget_stopped_in(&stopped_record_path(session));
}

/// The `__jobs` mode's entry: the process's cwd and session come from the
/// environment, the backend from [`BACKEND`].
pub fn cli_run(argv: &[OsString]) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("{NAME}: cannot read the working directory: {err}");
            return 1;
        }
    };
    let session = std::env::var(VAR_SESSION).ok();
    run_cli(argv, Path::new(BACKEND), &cwd, session.as_deref())
}

/// [`cli_run`] with the backend, the directory the backend scopes to, and
/// the session made explicit: the seam tests drive, since neither the cwd
/// nor the backend path can be changed process-wide in a parallel test
/// run.
///
/// The exit code is the process's: 0 success, 1 a backend failure, 2
/// usage, [`EXIT_NOT_WIRED`] outside a wired shell, [`EXIT_NO_BACKEND`]
/// when the backend binary is missing.
pub fn run_cli(argv: &[OsString], backend: &Path, cwd: &Path, session: Option<&str>) -> i32 {
    match argv.get(1).and_then(|verb| verb.to_str()) {
        Some("run") => run_command(&argv[2..], backend, cwd, session),
        Some("ls" | "list") => ls_command(&argv[2..], backend, cwd, session),
        Some("logs") => forward(backend, cwd, "logs", &argv[2..]),
        Some("stop") => stop_forward(backend, cwd, "stop", &argv[2..], session),
        Some("wait") => forward(backend, cwd, "await", &argv[2..]),
        Some("rm" | "remove") => stop_forward(backend, cwd, "remove", &argv[2..], session),
        Some("shutdown") => shutdown_command(backend, cwd, session),
        Some("help" | "--help" | "-h") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => {
            eprintln!("{NAME}: unknown verb `{other}`\n\n{USAGE}");
            2
        }
    }
}

/// `job run <line>`: start a shell line in the background, or refuse and
/// name the owner when the same line is already running here.
fn run_command(args: &[OsString], backend: &Path, cwd: &Path, session: Option<&str>) -> i32 {
    let Some(session) = session.filter(|session| !session.is_empty()) else {
        eprintln!("{NAME}: not wired in this shell (no {VAR_SESSION})");
        return EXIT_NOT_WIRED;
    };
    let command = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let command = command.trim();
    if command.is_empty() {
        eprintln!("{NAME}: run needs a command\n\n{USAGE}");
        return 2;
    }
    let jobs = match list_backend(backend, cwd, true) {
        Ok(jobs) => jobs,
        Err(code) => return code,
    };
    // The backend dedupes an identical running command by reusing it and
    // REWRITING its description, which would move the job from the session
    // that started it to this one. Check first, refuse instead, and say
    // who holds it. "Identical" is the backend's own rule: the same argv
    // in the same directory, which is why the workdir rides the compare.
    if let Some(running) = jobs.iter().find(|job| {
        job.status.starts_with("running")
            && unwrap_command(&job.command) == command
            && same_directory(job, cwd)
    }) {
        match owner_of(running.description.as_deref()) {
            Some(owner) if owner != session => println!(
                "already running as {} (started by session {owner}); \
                 not starting a second one",
                running.id
            ),
            _ => println!("already running as {}", running.id),
        }
        println!("  {NAME} logs {}\n  {NAME} stop {}", running.id, running.id);
        return 0;
    }
    let description = tagged(session, command);
    let output = Command::new(backend)
        .current_dir(cwd)
        .arg("add")
        .arg("--description")
        .arg(&description)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(command)
        .output();
    let id = match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            parse_added_id(&text).or_else(|| find_by_description(backend, cwd, &description))
        }
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stderr);
            eprintln!("{NAME}: the backend refused it: {}", text.trim());
            return 1;
        }
        Err(err) => {
            eprintln!("{NAME}: cannot run `{BACKEND}`: {err}");
            return EXIT_NO_BACKEND;
        }
    };
    let Some(id) = id else {
        // The job started but the listing did not show it yet: the
        // backend's success line is enough to tell the model where to look.
        println!("started `{command}` (id unknown; `{NAME} ls`)");
        return 0;
    };
    println!("started {id}: {command}");
    println!("  {NAME} logs {id}\n  {NAME} stop {id}");
    0
}

/// Whether a listing entry ran in `cwd`. A backend that does not report
/// the column leaves the command alone to decide, which is where this
/// check started.
fn same_directory(job: &BackendJob, cwd: &Path) -> bool {
    job.workdir
        .as_deref()
        .is_none_or(|dir| Path::new(dir) == cwd)
}

/// The id from the backend's success line: `Added job abc running: …`, or
/// `Job abc already running (since …)` when a race slipped past the
/// pre-check.
fn parse_added_id(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("Added"), Some("job")) => {
                if let Some(id) = words.next() {
                    return Some(id.to_owned());
                }
            }
            (Some("Job"), Some(id)) if line.contains("already running") => {
                return Some(id.to_owned());
            }
            _ => {}
        }
    }
    None
}

/// The id of the job carrying a description, for the backend output that
/// did not parse.
fn find_by_description(backend: &Path, cwd: &Path, description: &str) -> Option<String> {
    list_backend(backend, cwd, true)
        .ok()?
        .iter()
        .find_map(|job| (job.description.as_deref() == Some(description)).then(|| job.id.clone()))
}

/// `job ls`: this session's jobs by default; `--dir` is every job started
/// in this directory, `--all` everything on the machine. `--json` prints
/// the rows for whichever scope, which is what the frontend polls.
fn ls_command(args: &[OsString], backend: &Path, cwd: &Path, session: Option<&str>) -> i32 {
    let mut scope = Scope::Session;
    let mut json = false;
    let mut filter_session = session.map(str::to_owned);
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--all" | "-a") => scope = Scope::Machine,
            Some("--dir") => scope = Scope::Directory,
            Some("--json") => json = true,
            Some("--session") => {
                index += 1;
                filter_session = args
                    .get(index)
                    .map(|value| value.to_string_lossy().into_owned());
            }
            Some(other) => {
                eprintln!("{NAME}: ls does not take `{other}`");
                return 2;
            }
            None => return 2,
        }
        index += 1;
    }
    let rows = match owned_rows(backend, cwd, scope, filter_session.as_deref()) {
        Ok(rows) => rows,
        Err(code) => return code,
    };
    if json {
        match serde_json::to_string(&rows) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("{NAME}: cannot write the listing: {err}");
                return 1;
            }
        }
        return 0;
    }
    if rows.is_empty() {
        println!("no jobs");
        return 0;
    }
    for row in &rows {
        let exit = match row.exit_code {
            _ if row.status.starts_with("running") => String::new(),
            Some(code) => format!(" ({code})"),
            None => String::new(),
        };
        let owner = match (&row.session, scope == Scope::Session) {
            (Some(owner), false) => format!(" [{owner}]"),
            _ => String::new(),
        };
        println!("{} {}{exit}{owner}: {}", row.id, row.status, row.command);
    }
    0
}

/// What a listing covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// This session's jobs, wherever they were started: the tag is the
    /// ownership record, so a server started in a subdirectory still
    /// belongs to the strip that started it.
    Session,
    /// Everything started in this directory, any session, any shim.
    Directory,
    /// Everything on the machine.
    Machine,
}

/// The rows one listing prints. A job started outside the shim has no tag
/// and is never "ours", which keeps a hand-started `gob add` out of a
/// session's strip.
fn owned_rows(
    backend: &Path,
    cwd: &Path,
    scope: Scope,
    session: Option<&str>,
) -> Result<Vec<JobRow>, i32> {
    let jobs = list_backend(backend, cwd, scope != Scope::Directory)?;
    Ok(jobs
        .iter()
        .filter(|job| match scope {
            Scope::Session => match (session, owner_of(job.description.as_deref())) {
                (Some(want), Some(owner)) => want == owner,
                _ => false,
            },
            Scope::Directory | Scope::Machine => true,
        })
        .map(BackendJob::row)
        .collect())
}

/// `job shutdown`: stop every job this session started. Other sessions'
/// jobs, and jobs older than this session, stay.
fn shutdown_command(backend: &Path, cwd: &Path, session: Option<&str>) -> i32 {
    let Some(session) = session.filter(|session| !session.is_empty()) else {
        eprintln!("{NAME}: not wired in this shell (no {VAR_SESSION})");
        return EXIT_NOT_WIRED;
    };
    let jobs = match list_backend(backend, cwd, true) {
        Ok(jobs) => jobs,
        Err(code) => return code,
    };
    let mut stopped_count = 0;
    let mut failed = 0;
    let mut stopped: Vec<String> = Vec::new();
    for job in jobs.iter().filter(|job| {
        job.status.starts_with("running")
            && owner_of(job.description.as_deref()).as_deref() == Some(session)
    }) {
        match Command::new(backend)
            .current_dir(cwd)
            .arg("stop")
            .arg(&job.id)
            .output()
        {
            Ok(out) if out.status.success() => {
                stopped_count += 1;
                stopped.push(job.id.clone());
            }
            _ => failed += 1,
        }
    }
    // A job `shutdown` failed to stop is not recorded: whatever ends it
    // later was not the model's doing, and its finish still announces.
    record_stopped(Some(session), &stopped);
    println!("stopped {stopped_count} job(s)");
    if failed > 0 {
        eprintln!("{NAME}: {failed} job(s) would not stop");
        return 1;
    }
    0
}

/// One of the verbs with no shim logic: the backend's own command, its
/// own streams, its own exit code.
fn forward(backend: &Path, cwd: &Path, verb: &str, args: &[OsString]) -> i32 {
    match Command::new(backend)
        .current_dir(cwd)
        .arg(verb)
        .args(args)
        .status()
    {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("{NAME}: cannot run `{BACKEND}`: {err}");
            EXIT_NO_BACKEND
        }
    }
}

/// `job stop` and `job rm`: the backend's verb, and on success the
/// record. The args ride into the record as they came: a token that is
/// not an id the backend stopped never mints a finish anyway, and a
/// shape no parser could resolve (`job stop "$id"`) still passes
/// through the forward whole.
fn stop_forward(
    backend: &Path,
    cwd: &Path,
    verb: &str,
    args: &[OsString],
    session: Option<&str>,
) -> i32 {
    let code = forward(backend, cwd, verb, args);
    if code == 0 {
        let ids: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        record_stopped(session, &ids);
    }
    code
}

/// The backend's listing for one directory, parsed. The error is the exit
/// code the caller returns.
fn list_backend(backend: &Path, cwd: &Path, all_dirs: bool) -> Result<Vec<BackendJob>, i32> {
    let mut command = Command::new(backend);
    command.current_dir(cwd).arg("list").arg("--json");
    if all_dirs {
        command.arg("--all");
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            eprintln!("{NAME}: cannot run `{BACKEND}`: {err}");
            return Err(EXIT_NO_BACKEND);
        }
    };
    if !output.status.success() {
        eprintln!(
            "{NAME}: `{BACKEND} list` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(1);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // An empty listing is a legal answer, and a backend may print nothing
    // at all rather than `[]`.
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).map_err(|err| {
        eprintln!("{NAME}: cannot read `{BACKEND} list --json`: {err}");
        1
    })
}

#[cfg(test)]
mod tests;
