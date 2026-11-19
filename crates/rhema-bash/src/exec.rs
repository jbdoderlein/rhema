//! The execution seam behind the `bash` tool: where commands actually run.
//!
//! [`crate::shell::ShellTools`] owns the tool's contract, the spec, the
//! approval gate, the output caps, the timeout, the error shaping, and
//! delegates the run itself to a [`ShellExecutor`]. The local executor
//! spawns `bash -c` child processes (feature `local-shell`, default on);
//! hosts whose shell lives elsewhere bring their own executor, the cloud
//! deployment runs every command in a remote sandbox through this same
//! seam. Captured runs are never handed the frontend's terminal: the
//! local executor gives the child `/dev/null` and a session with no
//! controlling tty, so an interactive program fails instead of taking
//! over the UI. The trait's futures are `Send` like every seam in this
//! tree, so the executor object crosses await points freely.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "local-shell")]
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "local-shell")]
use tokio::io::{AsyncBufReadExt, BufReader};

/// What one command left behind: the captured streams and the exit code.
/// The streams carry whatever the executor captured; the tool, not the
/// executor, applies the display caps to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    /// Everything the command wrote to stdout.
    pub stdout: Vec<u8>,
    /// Everything the command wrote to stderr.
    pub stderr: Vec<u8>,
    /// The exit code; `None` when the process was terminated by a signal.
    pub code: Option<i32>,
}

/// Why a command did not reach an exit code. In-band: the tool turns
/// every variant into an `is_error` result for the model, never an abort.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellError {
    /// The command ran past its timeout and was killed.
    #[error("command timed out after {0}s and was killed")]
    Timeout(u64),
    /// The driver cancelled the command mid-run: the context's cancelled
    /// flag flipped before the command finished.
    #[error("cancelled")]
    Cancelled,
    /// The command never started, spawning failed, or the executor's
    /// transport to its host failed.
    #[error("failed to run bash: {0}")]
    Failed(String),
}

/// The bash executable the shell tool and the startup probe run under.
/// `RHEMA_SHELL` answers first, everywhere, a frontend provisioning a
/// known bash, a container, a nonstandard layout. Windows has no bash
/// of its own, so the resolution goes for Git Bash: the one bash most
/// windows dev boxes have, and what keeps the agent's unix vocabulary
/// true, the Git for Windows layouts first, then `bash` on PATH (a
/// git bash that put itself there, MSYS2, cygwin). Everywhere else,
/// plain `bash` on PATH.
pub fn bash_executable() -> PathBuf {
    if let Ok(shell) = std::env::var("RHEMA_SHELL")
        && !shell.trim().is_empty()
    {
        return shell.into();
    }
    #[cfg(windows)]
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(dir) = std::env::var(base) {
            let candidate = PathBuf::from(dir).join("Git").join("bin").join("bash.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("bash")
}

/// What a spawn that could not find bash reports: the fact plus the
/// one install that fixes it, so a windows session names the gap
/// instead of retrying unix commands into it. In-band: callers shape
/// this into an error the model reads.
pub fn bash_missing_message() -> String {
    if cfg!(windows) {
        "bash was not found on this windows host, install Git for \
         Windows (https://git-scm.com/download/win), or point \
         RHEMA_SHELL at a bash.exe"
    } else {
        "bash was not found on this host, install bash, or point \
         RHEMA_SHELL at the bash executable"
    }
    .to_owned()
}

/// The environment every captured command runs with, layered over the
/// inherited one: the variables that would otherwise make a tool wait on
/// a terminal the command does not have. Git's editor slot is the one
/// that bit in the field: `git commit` without `-m` launches
/// `$GIT_EDITOR` (micro, vim), which opens `/dev/tty` and takes the
/// frontend's screen and keyboard. `false` ends that editor at once, and
/// git's own error tells the model to pass `-m` or `-F`. A frontend's
/// provisioned env is layered over these, so an explicit choice wins.
pub const NONINTERACTIVE_ENV: &[(&str, &str)] = &[
    ("GIT_EDITOR", "false"),
    ("GIT_SEQUENCE_EDITOR", "false"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("DEBIAN_FRONTEND", "noninteractive"),
];

/// The `setsid` binary on this host, resolved from `PATH` once. A
/// captured command runs in a new session under it, so a program that
/// opens `/dev/tty` gets `ENXIO` instead of the frontend's terminal.
/// `None` on hosts without util-linux (macOS, Git Bash): there the
/// `/dev/null` stdin and [`NONINTERACTIVE_ENV`] are the whole guard.
#[cfg(unix)]
pub fn setsid_executable() -> Option<&'static Path> {
    use std::sync::OnceLock;
    static SETSID: OnceLock<Option<PathBuf>> = OnceLock::new();
    SETSID
        .get_or_init(|| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|directory| directory.join("setsid"))
                .find(|candidate| candidate.is_file())
        })
        .as_deref()
}

/// The non-unix counterpart: no `setsid` to find.
#[cfg(not(unix))]
pub fn setsid_executable() -> Option<&'static Path> {
    None
}

/// Shapes a spawn failure: not-found names the gap (see
/// [`bash_missing_message`]), anything else rides the io error text.
/// The shell's shape: the one caller is the local executor, and the
/// shell-less targets (wasm) keep neither.
#[cfg(feature = "local-shell")]
fn spawn_error(err: std::io::Error) -> ShellError {
    match err.kind() {
        std::io::ErrorKind::NotFound => ShellError::Failed(bash_missing_message()),
        _ => ShellError::Failed(format!("{err}")),
    }
}

/// Runs shell commands for the `bash` tool.
#[async_trait]
pub trait ShellExecutor: Send + Sync {
    /// Runs `command` with `bash -c` and `cwd` as working directory,
    /// streaming complete output lines through `progress` as they land
    /// and returning what the command left behind. A command outliving
    /// `timeout` is killed and reported as [`ShellError::Timeout`]; a
    /// flip of `cancelled` mid-run stops the run with
    /// [`ShellError::Cancelled`]. A captured run is non-interactive: the
    /// child never receives the frontend's terminal on stdin, and an
    /// implementation must not hand it one. Implementations must not
    /// panic and must not abort: every failure comes back as a
    /// [`ShellError`].
    async fn run(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
        progress: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Executed, ShellError>;
}

/// How often the cancelled flag is sampled while a command's output is
/// quiet, an `AtomicBool` carries no wakeup, so a silent command would
/// otherwise never observe a flip until its deadline.
#[cfg(feature = "local-shell")]
const CANCEL_TICK: Duration = Duration::from_millis(100);

/// One stream's capture window: everything the result renderer can
/// show (`BASH_OUTPUT_LIMIT` of bytes, or the last `BASH_OUTPUT_LINES`
/// rows, whichever cuts later) plus slack for line alignment, so the
/// truncation the model sees matches an uncaptured run's. Bounds the
/// memory a chatty command can pin: without it, `yes` streams through
/// the channel into these buffers until the deadline.
#[cfg(feature = "local-shell")]
const CAPTURE_WINDOW: usize = 50 * 1024 + 1024 * 1024;

/// The overshoot an eviction waits for: draining per line would shift
/// the retained window on every line of a chatty stream (quadratic);
/// waiting for a quarter-window of overshoot makes the cost
/// amortized-linear. Peak residency is the window plus this.
#[cfg(feature = "local-shell")]
const CAPTURE_EVICT: usize = 256 * 1024;

/// The local executor: commands run as `bash -c` child processes on this
/// host, streamed line by line, killed past their timeout (`kill_on_drop`
/// so the dropped child handle kills a still-running process). Captured
/// commands get no terminal: `/dev/null` on stdin, no controlling tty
/// under `setsid` where the host has it, and [`NONINTERACTIVE_ENV`] so
/// an editor or pager fails fast instead of taking the frontend's screen
/// and keyboard.
#[cfg(feature = "local-shell")]
#[derive(Debug, Clone)]
pub struct LocalShell {
    /// Extra environment every command runs with, layered over the
    /// inherited one, how a frontend provisions its agent's tools (a
    /// daemon URL, a scoped token, a PATH addition) without touching
    /// the process environment.
    env: Arc<[(String, String)]>,
}

#[cfg(feature = "local-shell")]
impl Default for LocalShell {
    fn default() -> Self {
        Self { env: Arc::from([]) }
    }
}

#[cfg(feature = "local-shell")]
#[async_trait]
impl ShellExecutor for LocalShell {
    async fn run(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
        progress: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Executed, ShellError> {
        use std::sync::atomic::Ordering;

        // kill_on_drop so the timeout dropping the child handle kills the
        // process; without it an expired command would keep running.
        let mut child = captured_command(command, cwd, &self.env)
            .spawn()
            .map_err(spawn_error)?;
        // One reader per stream, complete lines into one channel: each
        // line streams live through `progress` and lands in its
        // stream's tail for the result.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(stdout) = child.stdout.take() {
            pump(stdout, false, tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pump(stderr, true, tx.clone());
        }
        drop(tx);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(ShellError::Timeout(timeout.as_secs()));
                }
                line = rx.recv() => match line {
                    Some((is_stderr, line)) => {
                        let sink = if is_stderr { &mut stderr } else { &mut stdout };
                        sink.extend_from_slice(line.as_bytes());
                        sink.push(b'\n');
                        // Bound the capture: the renderer reads only
                        // the tail, so past the window drop whole head
                        // lines down to it. The cut lands on a newline
                        // when there is one, keeping the retained bytes
                        // line-framed for the renderer's tail logic.
                        if sink.len() >= CAPTURE_WINDOW + CAPTURE_EVICT {
                            let keep_from = sink.len() - CAPTURE_WINDOW;
                            let boundary = sink[keep_from..]
                                .iter()
                                .position(|&byte| byte == b'\n')
                                .map_or(keep_from, |position| keep_from + position + 1);
                            sink.drain(..boundary);
                        }
                        progress(&format!("{line}\n"));
                    }
                    // Both streams closed: only the exit status is left.
                    None => break,
                },
                // The cancelled flag carries no wakeup: without this tick
                // a command that streams nothing would not observe a flip
                // until its deadline. The tick is the cancellation check,
                // not a timer kill, the budget belongs to the deadline.
                _ = tokio::time::sleep(CANCEL_TICK) => {
                    if cancelled.load(Ordering::Relaxed) {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(ShellError::Cancelled);
                    }
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ShellError::Cancelled);
            }
        }
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => status,
            // A process that closed its streams but never exits dies with
            // the handle; the caller reads a timeout like any other.
            _ => return Err(ShellError::Timeout(timeout.as_secs())),
        };
        Ok(Executed {
            stdout,
            stderr,
            code: status.code(),
        })
    }
}

/// Builds the `bash -c` command for one captured run: no terminal on
/// stdin, [`NONINTERACTIVE_ENV`] under the frontend's own env, and
/// `setsid` in front when the host has it, so a program that reaches for
/// `/dev/tty` finds no controlling terminal instead of taking the
/// frontend's screen and keyboard. Without `setsid` the stdio contract
/// still holds: stdin is `/dev/null`, so a reader gets EOF rather than
/// the user's keys.
#[cfg(feature = "local-shell")]
fn captured_command(
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
) -> tokio::process::Command {
    use std::process::Stdio;

    let bash = bash_executable();
    let mut process = match setsid_executable() {
        Some(setsid) => {
            let mut process = tokio::process::Command::new(setsid);
            process.arg(&bash);
            process
        }
        None => tokio::process::Command::new(&bash),
    };
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        // The guards first, the frontend's env over them: a provisioned
        // value is an explicit choice and wins.
        .envs(NONINTERACTIVE_ENV.iter().copied())
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process
}

/// Reads `stream` line by line into `tx`, tagged with its stream, until
/// EOF, the live side of the command's output. Detached: it ends with
/// the pipe, whether the command finishes, times out, or is killed.
#[cfg(feature = "local-shell")]
fn pump<R>(
    stream: R,
    is_stderr: bool,
    tx: tokio::sync::mpsc::UnboundedSender<(bool, String)>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        // Lines are read as bytes under the same per-line cap the
        // sandbox pump applies: a command that never emits a newline
        // (`yes | tr -d '\n'`) would grow `lines`' buffer without
        // bound until the deadline. The overrun bytes are consumed
        // but not kept, and a marker line makes the elision visible,
        // so framing survives.
        let mut reader = BufReader::new(stream);
        let mut buffer: Vec<u8> = Vec::with_capacity(MAX_LINE_BYTES);
        'lines: loop {
            buffer.clear();
            let mut dropped = 0u64;
            let got_newline = loop {
                match reader.fill_buf().await {
                    // EOF: the pending bytes, if any, are a final
                    // line without its newline, deliver them below.
                    Ok([]) => break false,
                    Ok(available) => {
                        let newline = available.iter().position(|&byte| byte == b'\n');
                        let chunk_len = newline.unwrap_or(available.len());
                        let room = MAX_LINE_BYTES.saturating_sub(buffer.len());
                        let take = chunk_len.min(room);
                        buffer.extend_from_slice(&available[..take]);
                        dropped += (chunk_len - take) as u64;
                        let consumed = newline.map_or(available.len(), |pos| pos + 1);
                        reader.consume(consumed);
                        if newline.is_some() {
                            break true;
                        }
                    }
                    Err(_) => break 'lines,
                }
            };
            if !got_newline && buffer.is_empty() && dropped == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buffer);
            let text = text.strip_suffix('\r').unwrap_or(&text);
            if tx.send((is_stderr, text.to_string())).is_err() {
                break; // the resolver is gone; nothing to stream to
            }
            if dropped > 0 {
                let marker = format!("[line truncated, {dropped} bytes dropped]");
                if tx.send((is_stderr, marker)).is_err() {
                    break;
                }
            }
            if !got_newline {
                break;
            }
        }
    })
}

/// The most bytes one delivered line may hold in memory (see `pump`).
#[cfg(feature = "local-shell")]
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// The no-shell placeholder: every command fails in-band. What
/// `ShellTools::new` installs when the build carries no local shell,
/// the model sees the failure, the operator sees the missing setup.
#[cfg(not(feature = "local-shell"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoShell;

#[cfg(not(feature = "local-shell"))]
#[async_trait]
impl ShellExecutor for NoShell {
    async fn run(
        &self,
        _command: &str,
        _cwd: &Path,
        _timeout: Duration,
        _cancelled: &AtomicBool,
        _progress: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Executed, ShellError> {
        Err(ShellError::Failed(
            "this build has no local shell; install a shell executor \
             (ShellTools::with_executor)"
                .into(),
        ))
    }
}

/// The executor `ShellTools::new` installs: the local process shell where
/// the build has one, the in-band no-shell stub where it does not.
#[cfg(feature = "local-shell")]
pub fn default_executor() -> std::sync::Arc<dyn ShellExecutor> {
    std::sync::Arc::new(LocalShell::default())
}

/// The default executor with `env` layered over every command's
/// environment, the provisioning seam a frontend's agent tools ride
/// in on. A build without the `local-shell` feature has no commands to
/// provision; the stub ignores `env`.
#[cfg(feature = "local-shell")]
pub fn default_executor_with(env: &BTreeMap<String, String>) -> std::sync::Arc<dyn ShellExecutor> {
    std::sync::Arc::new(LocalShell {
        env: Arc::from(
            env.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>(),
        ),
    })
}

/// The stub counterpart of [`default_executor_with`]: nothing runs, so
/// there is nothing to provision.
#[cfg(not(feature = "local-shell"))]
pub fn default_executor_with(_env: &BTreeMap<String, String>) -> std::sync::Arc<dyn ShellExecutor> {
    std::sync::Arc::new(NoShell)
}

/// The executor `ShellTools::new` installs: the local process shell where
/// the build has one, the in-band no-shell stub where it does not.
#[cfg(not(feature = "local-shell"))]
pub fn default_executor() -> std::sync::Arc<dyn ShellExecutor> {
    std::sync::Arc::new(NoShell)
}

#[cfg(all(test, feature = "local-shell"))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn silent(_line: &str) {}

    /// One run through an executor, the deadline and the cwd the tests
    /// share; the output is the assertion, so a nonzero exit still
    /// comes back here.
    async fn run_collected(executor: &dyn ShellExecutor, command: &str) -> Executed {
        let cancelled = AtomicBool::new(false);
        executor
            .run(
                command,
                Path::new("/tmp"),
                Duration::from_secs(30),
                &cancelled,
                &mut silent,
            )
            .await
            .expect("the command runs")
    }

    #[tokio::test]
    async fn a_captured_command_reads_eof_instead_of_the_terminal() {
        let shell = LocalShell::default();
        // Without `Stdio::null` this reads the test harness's stdin: a
        // terminal in a local run, and the test blocks forever.
        let executed = run_collected(&shell, "read -r line; echo \"status:$?\"").await;
        assert_eq!(executed.code, Some(0));
        assert_eq!(String::from_utf8_lossy(&executed.stdout), "status:1\n");
    }

    #[tokio::test]
    async fn a_captured_command_carries_the_noninteractive_guards() {
        let shell = LocalShell::default();
        let executed = run_collected(
            &shell,
            "printf '%s|%s|%s|%s\\n' \"$GIT_EDITOR\" \"$GIT_SEQUENCE_EDITOR\" \"$GIT_PAGER\" \"$GIT_TERMINAL_PROMPT\"",
        )
        .await;
        assert_eq!(
            String::from_utf8_lossy(&executed.stdout),
            "false|false|cat|0\n"
        );
    }

    #[tokio::test]
    async fn a_frontend_env_overrides_a_guard() {
        let mut env = BTreeMap::new();
        env.insert("GIT_EDITOR".to_owned(), "true".to_owned());
        let executor = default_executor_with(&env);
        let executed = run_collected(executor.as_ref(), "printf '%s\\n' \"$GIT_EDITOR\"").await;
        assert_eq!(String::from_utf8_lossy(&executed.stdout), "true\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_captured_command_has_no_controlling_terminal_when_setsid_exists() {
        if setsid_executable().is_none() {
            return;
        }
        let shell = LocalShell::default();
        let executed = run_collected(
            &shell,
            "if { : </dev/tty; } 2>/dev/null; then echo attached; else echo detached; fi",
        )
        .await;
        assert_eq!(String::from_utf8_lossy(&executed.stdout), "detached\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_detached_wrapper_preserves_the_exit_code() {
        if setsid_executable().is_none() {
            return;
        }
        let shell = LocalShell::default();
        let executed = run_collected(&shell, "exit 7").await;
        assert_eq!(executed.code, Some(7));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_git_commit_without_a_message_never_launches_the_editor() {
        use std::os::unix::fs::PermissionsExt;

        let git_available = std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !git_available {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let setup = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("git runs");
            assert!(status.success(), "git {args:?}");
        };
        setup(&["init", "-q"]);
        std::fs::write(dir.path().join("f"), b"x").expect("the file is written");
        setup(&["add", "f"]);

        // The editor the environment names must never run: the guard in
        // NONINTERACTIVE_ENV wins over `$EDITOR`, and there is no tty
        // for it to take either way.
        let marker = dir.path().join("editor-ran");
        let editor = dir.path().join("editor.sh");
        std::fs::write(
            &editor,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .expect("the editor script is written");
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))
            .expect("the editor script is executable");

        let mut env = BTreeMap::new();
        env.insert("EDITOR".to_owned(), editor.display().to_string());
        env.insert("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned());
        env.insert("GIT_CONFIG_SYSTEM".to_owned(), "/dev/null".to_owned());
        let executor = default_executor_with(&env);
        let cancelled = AtomicBool::new(false);
        let executed = executor
            .run(
                "git -c user.name=t -c user.email=t@example.com commit",
                dir.path(),
                Duration::from_secs(30),
                &cancelled,
                &mut silent,
            )
            .await
            .expect("the command runs");
        assert_ne!(
            executed.code,
            Some(0),
            "git must abort without a message: {}",
            String::from_utf8_lossy(&executed.stderr)
        );
        assert!(!marker.exists(), "the editor was launched");
    }

    #[tokio::test]
    async fn a_chatty_command_cannot_grow_the_capture_without_bound() {
        let shell = LocalShell::default();
        let cancelled = Arc::new(AtomicBool::new(false));
        // ~1.8 MB of stdout: far past the capture window, far under
        // the deadline, so the cap, not the timeout, is what held.
        let executed = shell
            .run(
                "seq 1 300000",
                Path::new("/tmp"),
                Duration::from_secs(60),
                &cancelled,
                &mut silent,
            )
            .await
            .unwrap();
        assert_eq!(executed.code, Some(0));
        assert!(
            executed.stdout.len() < CAPTURE_WINDOW + CAPTURE_EVICT,
            "capture must stay bounded, got {} bytes",
            executed.stdout.len()
        );
        // The tail survives: the last lines are what the model reads.
        let tail = String::from_utf8_lossy(&executed.stdout);
        assert!(
            tail.ends_with("300000\n"),
            "tail lost: …{}",
            &tail[tail.len().saturating_sub(40)..]
        );
    }
}

#[cfg(all(test, feature = "local-shell"))]
mod bash_resolution_tests {
    use super::*;
    use std::io::ErrorKind;

    // `RHEMA_SHELL` unset, no Git Bash on a unix CI: the PATH name.
    #[cfg(unix)]
    #[test]
    fn bash_executable_defaults_to_the_path_name() {
        assert_eq!(bash_executable(), PathBuf::from("bash"));
    }

    #[test]
    fn a_missing_bash_names_the_gap_not_the_io_error() {
        let ShellError::Failed(message) = spawn_error(ErrorKind::NotFound.into()) else {
            panic!("a not-found spawn is a Failed error");
        };
        assert!(message.contains("bash was not found"), "{message}");
        #[cfg(windows)]
        assert!(message.contains("Git for Windows"), "{message}");
    }
}
