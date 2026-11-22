//! Property tests for the request view's invariants.
//!
//! The transcript's contract is structural, not numeric: `observe` keeps
//! every message, sections tile the prefix they replace without gaps or
//! overlap, the block is append-only except at consolidation, and between
//! flushes the request a caller sends is the previous request plus more
//! tail. These properties are checked over generated logs instead of one
//! hand-built scenario, so a failure shrinks to the few messages that
//! break it.

use hegel::TestCase;
use hegel::generators as gs;
use rhema_core::{Content, Message, Role, ToolCall, ToolResult};
use rhema_transcript::tree::Range;
use rhema_transcript::{Section, Transcript, TranscriptSettings};

/// A message shape broad enough to exercise the section rules: user and
/// assistant text, tool calls, and tool results (the block never opens
/// on one).
#[derive(Debug, Clone, hegel::DefaultGenerator)]
enum GenMessage {
    User(String),
    Assistant(String),
    Call {
        id: String,
        name: String,
        args: String,
    },
    Result {
        id: String,
        ok: bool,
        content: String,
    },
}

#[hegel::test_helper]
fn generated_messages(tc: &TestCase) -> Vec<Message> {
    let text = || gs::text().min_size(24).max_size(48);
    let generator = gs::default::<GenMessage>()
        .user(text())
        .assistant(text())
        .call(|g| g.id(text()).name(text()).args(text()))
        .result(|g| g.id(text()).ok(gs::booleans()).content(text()));
    let generated: Vec<GenMessage> = tc.draw(gs::vecs(generator).min_size(20).max_size(40));
    let mut messages: Vec<Message> = generated.into_iter().map(to_message).collect();
    // End on a small user turn: the planner never cuts the tail, so the
    // whole generated backlog is eligible for the block and every case
    // exercises the flush and render paths instead of passing through.
    messages.push(Message::text(Role::User, "continue"));
    messages
}

fn to_message(generated: GenMessage) -> Message {
    match generated {
        GenMessage::User(text) => Message::text(Role::User, text),
        GenMessage::Assistant(text) => Message::text(Role::Assistant, text),
        GenMessage::Call { id, name, args } => Message {
            annotations: Default::default(),
            role: Role::Assistant,
            content: vec![Content::ToolCall(ToolCall {
                id,
                name,
                args: serde_json::Value::String(args),
                provider_data: serde_json::Value::Null,
            })],
        },
        GenMessage::Result { id, ok, content } => Message {
            annotations: Default::default(),
            role: Role::Tool,
            content: vec![Content::ToolResult(if ok {
                ToolResult::success(id, content)
            } else {
                ToolResult::failure(id, content)
            })],
        },
    }
}

/// A small budget and block cap, so a handful of generated messages
/// engages the planner, the section step, and consolidation.
fn settings() -> TranscriptSettings {
    TranscriptSettings {
        max_view_bytes: 400,
        step_bytes: 120,
        max_section_messages: 4,
        max_block_bytes: 100,
        // A tight image cap too: the property suites exercise the
        // image bound's arithmetic alongside the byte bound's.
        max_view_images: 2,
    }
}

#[hegel::test]
fn the_view_keeps_the_logs_invariants(tc: TestCase) {
    let messages = generated_messages(&tc);
    let mut transcript = Transcript::new(settings());
    let mut log: Vec<Message> = Vec::with_capacity(messages.len());
    let mut previous_view: Option<Vec<Message>> = None;
    let mut previous_sections: Vec<Section> = Vec::new();
    let mut previous_end = 0usize;

    for message in messages {
        log.push(message);

        let before = transcript.stats();
        let view = transcript.view(&log);
        let after = transcript.stats();
        let sections = transcript.saved_sections();

        // The log is complete and unchanged.
        assert_eq!(transcript.len(), log.len(), "the mirror dropped a message");
        for (id, original) in log.iter().enumerate() {
            assert_eq!(
                transcript.message(id),
                Some(original),
                "message {id} changed in the mirror"
            );
        }

        // Sections tile `[0, end)` without gaps, overlap, empty ranges,
        // or a range past the log.
        let mut end = 0usize;
        for section in &sections {
            assert_eq!(
                section.range.id, end,
                "sections do not tile: gap or overlap at {end}"
            );
            assert!(section.range.n >= 1, "a section covers no messages");
            end = section
                .range
                .id
                .checked_add(section.range.n)
                .expect("a section range overflows usize");
            assert!(end <= log.len(), "a section reaches past the log");
        }

        // The view is either the whole log (no compaction) or the block
        // followed by exactly the messages after the sections.
        match &view {
            None => {
                assert!(sections.is_empty(), "a pass-through view carried sections");
                assert_eq!(end, 0, "a pass-through view left a sectioned prefix");
            }
            Some(view) => {
                assert!(!sections.is_empty(), "a compacted view carried no sections");
                assert_eq!(
                    view.len(),
                    1 + log.len() - end,
                    "the view lost or added messages"
                );
                assert_eq!(&view[1..], &log[end..], "the band is not the log's tail");
                let text = match view[0].content.first() {
                    Some(Content::Text(text)) => text,
                    other => panic!("the block is not text: {other:?}"),
                };
                assert!(text.starts_with("<transcript>"), "the block lost its head");
                assert!(text.ends_with("</transcript>"), "the block lost its tail");
                // The block renders every committed section in order,
                // header, text, and the blank line that separates them.
                let mut cursor = 0;
                for section in &sections {
                    let chunk = format!(
                        "{}+{}|\n{}\n\n",
                        section.range.id, section.range.n, section.text
                    );
                    let found = text[cursor..].find(&chunk).unwrap_or_else(|| {
                        panic!(
                            "the block omits or reorders the section {}+{}",
                            section.range.id, section.range.n
                        )
                    });
                    cursor += found + chunk.len();
                }
            }
        }

        let consolidated = after.consolidations != before.consolidations;
        let flushed = after.flushes != before.flushes;

        // A committed section never changes or moves, except when a
        // consolidation rewrites the block.
        if !consolidated {
            assert!(
                sections.starts_with(&previous_sections),
                "a committed section changed without a consolidation"
            );
            assert!(
                end >= previous_end,
                "the sectioned prefix shrank without a consolidation"
            );
        }

        // Between flushes the request prefix is byte-stable: the new
        // view starts with the old one and only appends tail messages.
        if !flushed && !consolidated {
            match (&previous_view, &view) {
                (Some(previous), Some(current)) => {
                    assert!(
                        current.len() >= previous.len(),
                        "the view shrank without a flush"
                    );
                    assert_eq!(
                        &current[..previous.len()],
                        &previous[..],
                        "the request prefix changed without a flush"
                    );
                }
                (None, None) => {}
                (None, Some(_)) => panic!("a view appeared without a flush"),
                (Some(_), None) => panic!("a view disappeared"),
            }
        }

        previous_view = view;
        previous_sections = sections;
        previous_end = end;
    }

    assert!(
        !transcript.saved_sections().is_empty(),
        "the generated log never engaged the planner"
    );

    // A checkpoint carries the sections, and a fresh mirror adopts them
    // when they tile a prefix. The resumed request must match the one
    // the original mirror would send now, not just its shape.
    let saved = transcript.saved_sections();
    let expected = transcript.view(&log);
    let mut resumed = Transcript::new(settings());
    resumed.adopt_sections(saved);
    let actual = resumed.view(&log);
    assert_eq!(
        actual, expected,
        "a resumed mirror rendered a different view"
    );
    assert_eq!(
        resumed.saved_sections(),
        transcript.saved_sections(),
        "a resumed mirror committed different sections"
    );
}

#[hegel::test]
fn range_reads_exactly_the_log_slice(tc: TestCase) {
    let messages = generated_messages(&tc);
    let mut transcript = Transcript::new(settings());
    transcript.observe(&messages);

    let id = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(messages.len().saturating_add(4)),
    );
    let n = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(messages.len().saturating_add(4)),
    );
    let expected = id.checked_add(n).and_then(|end| messages.get(id..end));
    assert_eq!(
        transcript.range(Range { id, n }),
        expected,
        "range({id}, {n}) did not read the log's slice"
    );

    // The range arrives from a tool call, so the sum is checked rather
    // than wrapped or panicked on.
    for (id, n) in [(usize::MAX, 1), (usize::MAX - 1, 2), (1, usize::MAX)] {
        assert_eq!(
            transcript.range(Range { id, n }),
            None,
            "range({id}, {n}) wrapped instead of refusing"
        );
    }
}
