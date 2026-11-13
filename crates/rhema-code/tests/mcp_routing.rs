//! The real binary's road for the MCP CLI. In production the agent
//! shell's generated functions exec the `rhema` binary itself
//! (`RHEMA_MCP_BIN` is the running frontend), never the standalone
//! `rhema-mcp-cli` — so the property under test is that this binary
//! hands `mcp`, `__server`, and every unrecognized subcommand to
//! `rhema_mcp::cli::run` and adopts its exit code, instead of the
//! outer clap shadowing the modes with its own subcommands or its
//! own `--help`.
//!
//! Outside a wired shell (`RHEMA_MCP_BRIDGE` and friends absent) the
//! bridge verbs exit 4 with the explanation spec §10 asks for on
//! stderr. An unknown mode rides the same catch-all road and exits 2,
//! the CLI's usage fault, with its `unknown mode` line on stderr.
//! Either routing break would look different: clap's own
//! unrecognized-subcommand error also exits 2 but prints usage text,
//! and a swallowed `--help` would exit 0.

use std::process::Command;

/// Runs the real binary with the given argv, stripped of the bridge
/// env so the CLI answers from outside a wired shell.
fn run_rhema(args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rhema"));
    command.args(args);
    for var in [
        "RHEMA_MCP_BRIDGE",
        "RHEMA_MCP_TOKEN",
        "RHEMA_MCP_TMP",
        "BASH_ENV",
    ] {
        command.env_remove(var);
    }
    command.output().expect("the rhema binary runs")
}

/// `rhema mcp servers` reaches the CLI's meta mode: its transport
/// fault is the process's exit code, and nothing clap-shaped leaks
/// into stdout.
#[test]
fn the_mcp_meta_verb_routes_to_the_mcp_cli() {
    let output = run_rhema(&["mcp", "servers"]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "exit 4 is the CLI's transport fault; a shadowed verb would differ. \
         stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no wired session"),
        "the explanation spec §10 asks for: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `rhema __server <name> --help` lands in the catch-all with its
/// flags verbatim: the CLI answers (transport fault outside a wired
/// shell), and the outer clap never claims the `--help` for itself.
#[test]
fn the_dunder_server_road_routes_to_the_mcp_cli() {
    let output = run_rhema(&["__server", "validate-test-mcp", "--help"]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "exit 4 is the CLI's transport fault; clap claiming the --help would exit 0. \
         stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no wired session"),
        "the explanation spec §10 asks for: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Any unrecognized subcommand follows the same road: the CLI owns
/// the mode namespace's tail, its usage fault and its stderr line.
#[test]
fn an_unrecognized_subcommand_follows_the_same_road() {
    let output = run_rhema(&["definitely-not-a-verb"]);
    assert_eq!(output.status.code(), Some(2), "the CLI's usage fault");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown mode"),
        "the CLI's stderr line, not clap's usage text: {stderr}"
    );
}
