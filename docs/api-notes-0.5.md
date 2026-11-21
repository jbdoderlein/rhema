# API notes for 0.5.0

Findings from the 0.4.0 review batch. Items 1-4 landed on main after the tag; the signatures opened, and these notes keep the outcome on record. Items 5-6 remain open, still breaking, still waiting for the 0.5.0 window. A version bump has not been made yet: the workspace says 0.4.1, and the landed batch is the first half of the breaking work.

## 1. One shape for "run a command" (landed)

Was three signatures for one operation:

- `Sandbox::exec(request: ExecRequest, cancelled, progress)` (rhema-sandbox/src/lib.rs:136)
- `ShellExecutor::run(command, cwd, timeout, cancelled, progress)` (rhema-bash/src/exec.rs:165, five positional args)
- the `ExecCall { command, cwd, timeout_secs }` wire DTO (rhema-sandbox/src/protocol.rs:41)

Landed as: `RunRequest { command, cwd: PathBuf, timeout }` / `RunOutcome` in rhema-bash/src/exec.rs, re-exported by rhema-sandbox; both seams take the struct and return the outcome, and both methods are named `run`. `ExecRequest`/`ExecOutcome`/`Executed` are gone; `ExecCall`/`ExecReply` remain as the wire DTOs, and `From<&RunRequest>` bridges to `ExecCall`. The adapter's rebuild-and-copy body reduced to one forwarding call.

## 2. The progress closure's HRTB forced `fn` workarounds (landed)

`impl FnMut(&str) + Send` (or the dyn form `&mut (dyn for<'a> FnMut(&'a str) + Send)`) could not be satisfied by a plain closure; callers wrote dummy `fn silent(_: &str) {}` workarounds (three copies in-tree).

Landed as: the sandbox and executor seams take `&mut dyn ToolProgress` (rhema-core's trait, reused rather than a new `ProgressCallback`), re-exported from rhema-sandbox so the backends keep one import point. The shell tool passes its own channel straight through; the shim closure in shell.rs is gone. A no-op listener is `OutputOnly(|_| {})`.

## 3. One error vocabulary at the sandbox boundary (landed)

Landed as: `SandboxError::OutOfMemory` and `ShellError::OutOfMemory`, so a SIGKILL from outside the budget keeps its class through the adapter instead of collapsing into a generic failure; `exec_util::classify_exit` (the old `exit_status`) is the one exit-code classifier, 137 included, and the backends route through it. `From<std::io::Error> for SandboxError` is the one construction path for bare transport errors; the context-rich `Unavailable(format!(...))` sites stay, they name what failed. `MonitorError`/`ChatError*` no longer existed by the time this landed.

## 4. Cap constants renamed after what they cap (landed)

`MAX_STREAM_BYTES`/`MAX_CAPTURE_BYTES`/`MAX_LINE_BYTES` are now `EXEC_TAIL_BYTES`, `EXEC_CAPTURE_BYTES`, `EXEC_LINE_BYTES`. One correction to the original plan: the single definition of the line cap lives in rhema-bash (its pump uses it, and rhema-sandbox depends on rhema-bash, not the other way), with `exec_util` re-exporting it; the other two are exec_util's own. rhema-mcp's `MAX_LINE_BYTES` (4 MiB, MCP framing) is untouched; it caps a different layer.

## 5. `NONINTERACTIVE_ENV` re-export chain (open)

rhema-sandbox's exec_util re-exports `NONINTERACTIVE_ENV`/`setsid_executable` from rhema-bash (exec_util.rs:65). The three backends (local/ssh/docker) import them across crates, so either name breaking hits all.

Fix at 0.5.0: hoist both to rhema-core (or a shared rhema-process crate) and re-export from there; `exec_util` keeps no second public path.

## 6. Tougher closure bounds (open, mostly moot)

The `progress` HRTBs item 2 complained about are gone. The retry ladder's callbacks have the same shape; applying `ToolProgress`-style naming there is the remaining piece.

## Verified clean in this batch (no action needed)

The five crate-level READMEs with code (rhema-code, rhema-session-store, -file, -sqlite, rhema-submissions) verified against source: 0 broken commands, 0 non-compiling examples, 0 phantom features. The only doc edits made are the two in rhema-code's README (the `background: native` default and the fluff sentence). The install-block URL is network-form and was not reachable from this host; it matches `DIST_BASE`/update.rs and the release pipeline owns it.

`cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features` pass after the 1-4 batch.
