//! Binary wiring only: parse args, run the TUI, report one error line.
//! Terminal setup and teardown live in `rhema::run`.

#![forbid(unsafe_code)]

use clap::Parser;

use rhema::Args;

/// How long a quit waits for work still on the blocking pool. A
/// background fetch can hold a blocking thread in a call nothing can
/// cancel (`getaddrinfo` against a host the resolver never answers, a
/// child that ignores its kill); waiting for it turns a quit into the
/// resolver's own timeout at the shell prompt. Past this, the exit
/// takes the threads with it.
const BLOCKING_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);

fn main() {
    let args = Args::parse();
    // `#[tokio::main]` would drop the runtime after `run` returns, and
    // that drop waits for the blocking pool without a bound. The
    // explicit runtime lets the exit bound that wait instead.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the tokio runtime builds");
    runtime.block_on(async {
        if let Err(err) = rhema::run(args).await {
            // One clean line on stderr; non-zero exit, 2 when the model
            // backend failed (a wrapper retries on its own schedule), 1
            // otherwise. The terminal is already restored by the time
            // `run` returns.
            eprintln!("error: {err:#}");
            std::process::exit(rhema::exit_code(&err));
        }
    });
    // The shutdown marks the blocking pool closed, so the runtime's
    // later drop does not wait again.
    runtime.shutdown_timeout(BLOCKING_DRAIN);
}
