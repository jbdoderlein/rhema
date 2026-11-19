# API notes deferred to 0.5

Findings from the 0.4.0 review batch that are real but too large to land before the tag. None block the release; each is an API-shape change for 0.5.0, when the published signatures next open up.

## 1. One shape for "run a command"

Three signatures for one operation, frozen by 0.4.0:

- `Sandbox::exec(request: ExecRequest, cancelled, progress, deadline)` (rhema-sandbox/src/lib.rs:136)
- `ShellExecutor::run(command, cwd, timeout, cancelled, progress)` (rhema-bash/src/exec.rs:165, five positional args)
- the `ExecCall { command, cwd, timeout_secs }` wire DTO (rhema-sandbox/src/protocol.rs:41)

Fix at 0.5.0: define `RunRequest { command, cwd, timeout }` / `RunOutcome`, convert via `From`, and take the struct as the single argument everywhere. Keep `ExecCall`/`ExecReply` only as wire DTOs.

## 2. The progress closure's HRTB forces `fn` workarounds

`impl FnMut(&str) + Send` (or the dyn form `&mut (dyn for<'a> FnMut(&'a str) + Send)`) cannot be satisfied by a plain `||` closure; every caller writes a dummy `fn silent(_: &str) {}` and passes `&mut silent`. Three copies in-tree: rhema-bash/src/exec.rs:494, rhema-bash/src/shell.rs:285, rhema-cloud-agent/src/gate.rs:106.

Fix at 0.5.0: name a `ProgressCallback` trait (like `Approval` in rhema-bash), or accept `impl FnMut(&str) + Send` so a buffer-closing closure works.

## 3. One error vocabulary at the sandbox boundary

`SandboxError` is the carrier for `exec()`/`monitor()`, but backends convert through per-layer maps, and the adapter collapses `Timeout` into a generic failure, losing the backoff hint. `map_err_exit` re-implementations exist per backend; the string version at exec_util.rs:220 has drifted from exec_util.rs:326's classifier.

Fix at 0.5.0: one `SandboxError` enum with a variant per failure class (timeout, OOM/137, transport, failed); `classify_exit` as the single public constructor; drop `MonitorError`/`ChatError*` and the per-backend `map_err` copies.

## 4. Cap constants: name them after what they cap

`MAX_STREAM_BYTES` (exec_util.rs:31, output tail), `MAX_CAPTURE_BYTES` (exec_util.rs:72, capture buffer), `MAX_LINE_BYTES` (exec_util.rs:264 pub; exec.rs:425 private re-declaration, same value, 1 MiB). Two of three are re-exported; the third is a private duplicate.

Fix at 0.5.0: `EXEC_TAIL_BYTES`, `EXEC_CAPTURE_BYTES`, `EXEC_LINE_BYTES`, one definition in rhema-sandbox, bash re-exports the names it uses rather than re-declaring values.

## 5. `NONINTERACTIVE_ENV` re-export chain

rhema-sandbox's exec_util re-exports `NONINTERACTIVE_ENV`/`setsid_executable` from rhema-bash (exec_util.rs:65). The three backends (local/ssh/docker) import them across crates, so either name breaking hits all.

Fix at 0.5.0: hoist both to rhema-core (or a shared rhema-process crate) and re-export from there; `exec_util` keeps no second public path.

## 6. Tougher closure bounds

`progress` HRTBs force the workarounds in item 2; the retry ladder's callbacks have the same shape.

Fix at 0.5.0: same `ProgressCallback` trait as item 2, applied to the ladder's `on_retry` and the monitor's `on_event`.

## Verified clean in this batch (no action needed)

The five crate-level READMEs with code (rhema-code, rhema-session-store, -file, -sqlite, rhema-submissions) verified against source: 0 broken commands, 0 non-compiling examples, 0 phantom features. The only doc edits made are the two in rhema-code's README (the `background: native` default and the fluff sentence). The install-block URL is network-form and was not reachable from this host; it matches `DIST_BASE`/update.rs and the release pipeline owns it.

`cargo check --workspace --all-targets` passes. The `#[non_exhaustive]` matches across Event/StopReason/TransportError/SearchError/SandboxError all carry fallback arms, and the events.rs panic arm's containment is now documented at its arm.
