//! Ready-made [`EventSink`]s: [`NoopSink`] for headless runs and
//! [`Console`] for a terminal demo or one-shot.

use std::collections::HashMap;
use std::io::{self, Write};

use rhema_core::{Event, EventSink};

/// Drops every event. The sink for a run that only needs the final
/// [`ModelOutput`](rhema_core::ModelOutput), and the one tests use.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&mut self, _event: Event<'_>) {}
}

/// Streams a turn to stdout: assistant text as it arrives, one line per
/// tool call and its outcome, a note while a retry waits.
///
/// Enough for a demo or a one-shot terminal run. A UI that redraws
/// regions implements [`EventSink`] itself.
pub struct Console {
    out: io::Stdout,
    /// Tool names by call id: `ToolResult` carries the id alone.
    calls: HashMap<String, String>,
}

impl Console {
    /// Writes to this process's stdout.
    pub fn stdout() -> Self {
        Self {
            out: io::stdout(),
            calls: HashMap::new(),
        }
    }

    fn line(&mut self, line: &str) {
        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.flush();
    }
}

impl EventSink for Console {
    fn emit(&mut self, event: Event<'_>) {
        match event {
            Event::TextDelta(text) => self.line(text),
            Event::ToolCallStart { id, name } => {
                self.calls.insert(id.to_owned(), name.to_owned());
                self.line(&format!("\n[{name}]\n"));
            }
            Event::ToolResult { id, is_error, .. } => {
                let name = self.calls.remove(id).unwrap_or_else(|| "tool".to_owned());
                let outcome = if is_error { "failed" } else { "ok" };
                self.line(&format!("[{name}: {outcome}]\n"));
            }
            Event::RetryWait {
                delay,
                attempt,
                total,
                ..
            } => self.line(&format!(
                "[retrying in {:.0}s, attempt {attempt}/{total}]\n",
                delay.as_secs_f64()
            )),
            _ => {}
        }
    }
}
