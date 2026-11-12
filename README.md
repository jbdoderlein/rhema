<div align="center">
<b>ῥῆμα, "the spoken word": words that do things.</b>
</div>

<br>

<div align="center">
<img src=".github/img/a1-breath-dark.png" width="100">

<br>

<a href="https://rhema.rs">rhema.rs</a>
</div>

<br>

<h1 align="center">Rhema</h1>

<br>

Rhema is a small, embeddable agent framework in Rust.

It consists of multiple components, going from `rhema-code`, a terminal-based coding agent frontend, down to `rhema-core`, a simple library allowing step-based interactions with LLMs.

## Components

<img align="center" src=".github/img/graph.png">

Each crate under `crates/` has its own README with its scope and usage; the workspace manifest lists the members.

### Rhema Code (Terminal UI)

<img align="center" src=".github/img/rhema-code.png">

Get Rhema Code (Linux, macOS, and Windows):
```
curl -fsSL https://get.rhema.rs/code | sh                  # Linux, macOS, git bash
irm https://get.rhema.rs/code.ps1 | iex                    # Windows PowerShell
```

On Windows, the agent's shell commands run through Git Bash — install
[Git for Windows](https://git-scm.com/download/win) first.

See <a href="./crates/rhema-code/README.md">Rhema Code README.md</a> for more.

### Rhema Claw

Still Work-In-Progress.
Rhema Claw is the resident tier of the framework: a claw-shaped daemon
that watches forges and calendar feeds, dispatches work to its agents,
and serves a chat surface over HTTP — with a web UI and an Android
client talking to it. It lives in its own repository:
<https://git.vhaudiquet.fr/vhaudiquet/rhema-claw>.

## rhema-core

Here is a small example code using `rhema-core` to interact with a model:

```rust
use std::sync::Arc;

use rhema_core::{
    async_trait, run_turn, Agent, AgentError, ChatParams, Content, Event,
    EventSink, Message, MockProvider, ModelOutput, Role, Session, StopReason,
    ToolCall, ToolContext, ToolResolver, ToolResult, ToolSpec, Usage,
};

struct Echo;

#[async_trait]
impl ToolResolver for Echo {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec::new("echo", "Echo the input back", serde_json::json!({}))]
    }
    async fn resolve(&self, call: ToolCall, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success(&call.id, call.args.to_string())
    }
}

// A sink that prints every event; replace with your UI/log/channel.
struct Print;
impl EventSink for Print {
    fn emit(&mut self, event: Event<'_>) { println!("{event:?}"); }
}

// Any ModelProvider implementation; MockProvider is the scripted offline
// one (feature `mock`). A real HTTP adapter is a rhema-provider-* crate.
fn provider() -> Arc<dyn rhema_core::ModelProvider> {
    Arc::new(MockProvider::new(vec![ModelOutput {
        message: Message {
            role: Role::Assistant,
            content: vec![Content::Text("done".into())],
        },
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }]))
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let mut agent = Agent::builder()
        .provider(provider())
        .resolver(Arc::new(Echo))
        .system("you are a helpful agent")
        .params(ChatParams::new("model-id")) // any id your provider accepts; the mock ignores it
        .build()?;
    let mut session = Session::default();
    let ctx = ToolContext::default();
    let input = Message::text(Role::User, "run the echo tool, then summarize");
    let output = run_turn(&mut agent, &mut session, input, &mut Print, &ctx, 8).await?;
    println!("{output:?}");
    Ok(())
}
```

Offline demo with zero network (the mock replays a scripted exchange):

```sh
cargo run -p rhema-core --features mock --example step_demo
```
