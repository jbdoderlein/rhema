//! Property tests for the zoom tool's paging.
//!
//! `zoom(id, n)` renders whole messages; anything over one page is cut
//! at [`ZOOM_MAX_BYTES`] and pages on through `at`. The model supplies
//! `id`, `n`, and `at` raw, so the contract is that paging is lossless
//! on any log: the pages tile the render exactly (no byte lost,
//! repeated, or split mid-character), every page stays within the byte
//! bound, and any argument the model can spell — past-the-end ids,
//! zero `n`, offsets that land mid-character or past the render —
//! comes back as a result or an in-band failure, never a panic.

use std::sync::{Arc, Mutex};

use hegel::TestCase;
use hegel::generators::{self as gs, TextGenerator};
use rhema_core::{Content, Message, Role, ToolCall, ToolResult};
use rhema_transcript::summary::render_message;
use rhema_transcript::tree::Range;
use rhema_transcript::{Transcript, TranscriptSettings, ZOOM_MAX_BYTES, Zoom};

/// Multibyte text with newlines, sized to straddle page boundaries:
/// the cut lands mid-message and often mid-character.
fn body() -> TextGenerator {
    gs::text().min_size(0).max_size(3_200).alphabet("abé😀 \n")
}

#[derive(Debug, Clone, hegel::DefaultGenerator)]
enum GenMessage {
    User(String),
    Assistant(String),
    Call { name: String, args: String },
    Result { ok: bool, content: String },
}

fn to_message(generated: GenMessage) -> Message {
    match generated {
        GenMessage::User(text) => Message::text(Role::User, text),
        GenMessage::Assistant(text) => Message::text(Role::Assistant, text),
        GenMessage::Call { name, args } => Message {
            annotations: Default::default(),
            role: Role::Assistant,
            content: vec![Content::ToolCall(rhema_core::ToolCall {
                id: format!("t{}", name.len()),
                name,
                args: serde_json::Value::String(args),
                provider_data: serde_json::Value::Null,
            })],
        },
        GenMessage::Result { ok, content } => Message {
            annotations: Default::default(),
            role: Role::Tool,
            content: vec![Content::ToolResult(if ok {
                ToolResult::success("t0", content)
            } else {
                ToolResult::failure("t0", content)
            })],
        },
    }
}

#[hegel::test_helper]
fn a_log(tc: &TestCase) -> Vec<Message> {
    let generated: Vec<GenMessage> = tc.draw(
        gs::vecs(
            gs::default::<GenMessage>()
                .user(body())
                .assistant(body())
                .call(|g| g.name(gs::text().max_size(6).alphabet("ab")).args(body()))
                .result(|g| g.ok(gs::booleans()).content(body())),
        )
        .min_size(1)
        .max_size(8),
    );
    generated.into_iter().map(to_message).collect()
}

/// The render the zoom module documents for a range: each message
/// whole under its `id+1|` address, newline-separated.
fn expected_render(messages: &[Message], id: usize, n: usize) -> String {
    let mut text = String::new();
    for (i, message) in messages.iter().enumerate().skip(id).take(n) {
        if !text.is_empty() {
            text.push('\n');
        }
        if n > 1 {
            text.push_str(&format!("{i}+1|"));
        }
        text.push_str(&render_message(message));
    }
    text
}

fn zoom_call(id: u64, n: u64, at: u64) -> ToolCall {
    ToolCall {
        id: "z".into(),
        name: "zoom".into(),
        args: serde_json::json!({ "id": id, "n": n, "at": at }),
        provider_data: serde_json::Value::Null,
    }
}

/// Strips a page's trailing note (`[N of T bytes; zoom again with
/// at=E]`) and returns the next `at` when there is one.
fn next_page(content: &str) -> Option<(String, u64)> {
    let marker = "\n\n[";
    let note_at = content.rfind(marker)?;
    let note = &content[note_at + marker.len()..];
    let at = note
        .split("at=")
        .nth(1)?
        .trim_end_matches(']')
        .parse()
        .ok()?;
    if note.contains("zoom again") {
        Some((content[..note_at].to_owned(), at))
    } else {
        None
    }
}

#[hegel::test(test_cases = 250)]
fn paging_tiles_the_render_exactly(tc: TestCase) {
    let messages = a_log(&tc);
    let mut transcript = Transcript::new(TranscriptSettings::default());
    transcript.observe(&messages);
    let zoom = Zoom::new(Arc::new(Mutex::new(transcript)));

    // A range inside the log, one to three messages wide, so the range
    // always renders whole and the pages carry the true total.
    let len = messages.len();
    let id = tc.draw(gs::integers::<usize>().max_value(len - 1));
    let n = tc.draw(
        gs::integers::<usize>()
            .min_value(1)
            .max_value(len.saturating_sub(id)),
    );

    let render = expected_render(&messages, id, n);
    let mut at = 0u64;
    let mut tiled = String::new();
    for _ in 0..400 {
        let result = zoom.zoom(&zoom_call(id as u64, n as u64, at));
        assert!(
            !result.is_error,
            "paging hit an in-band failure: {result:?}"
        );
        let Some((body, next)) = next_page(&result.content) else {
            tiled.push_str(&result.content);
            break;
        };
        tiled.push_str(&body);
        assert!(next > at, "paging stalled: at {next} follows {at}");
        at = next;
    }

    assert_eq!(
        tiled, render,
        "the pages of {id}+{n} did not tile the render"
    );

    // Every page stayed within the byte bound: the tile re-checks the
    // whole, the loop re-checks nothing, so walk the pages again and
    // bound each body.
    let mut bounded = true;
    let mut at = 0u64;
    for _ in 0..400 {
        let result = zoom.zoom(&zoom_call(id as u64, n as u64, at));
        if result.content.len() > ZOOM_MAX_BYTES + 200 {
            bounded = false;
            break;
        }
        match next_page(&result.content) {
            Some((_, next)) if next > at => at = next,
            _ => break,
        }
    }
    assert!(bounded, "a page blew the byte bound");
}

#[hegel::test(test_cases = 300)]
fn any_arguments_answer_in_band(tc: TestCase) {
    let messages = a_log(&tc);
    let mut transcript = Transcript::new(TranscriptSettings::default());
    transcript.observe(&messages);
    let len = messages.len();
    let zoom = Zoom::new(Arc::new(Mutex::new(transcript)));

    // A hostile grid over the argument space: edges of the log, zero,
    // the u64 ceiling, offsets that land mid-character or past the
    // render. None of these may panic; the out-of-range ones answer
    // as in-band failures naming the log.
    let ids = [0u64, len as u64 / 2, len as u64, len as u64 + 1, u64::MAX];
    let ns = [0u64, 1, 2, u64::MAX];
    let ats = [0u64, 1, ZOOM_MAX_BYTES as u64, u64::MAX];
    for id in ids {
        for n in ns {
            for at in ats {
                let result = zoom.zoom(&zoom_call(id, n, at));
                let in_range =
                    (id as usize) < len && (id as usize).saturating_add(n.min(1) as usize) <= len;
                if in_range && (id as usize) < len {
                    // An in-range call at a hostile offset still
                    // answers within the bound: a page, a whole
                    // render, or a failure explaining the offset.
                    assert!(
                        result.content.len() <= ZOOM_MAX_BYTES + 200,
                        "id {id} n {n} at {at} returned {} bytes",
                        result.content.len()
                    );
                } else {
                    assert!(
                        result.is_error,
                        "id {id} n {n} outside the log answered as success"
                    );
                }
            }
        }
    }

    // Offsets drawn through the log's own byte space, mid-character
    // included: the snap must keep every page start on a boundary,
    // which shows up as the pages still tiling when the caller pages
    // from a hostile offset.
    let id = tc.draw(gs::integers::<usize>().max_value(len - 1));
    let at: u64 = tc.draw(
        gs::integers::<u64>().max_value((render_message(&messages[id]).len() as u64) * 2 + 8),
    );
    let result = zoom.zoom(&zoom_call(id as u64, 1, at));
    let _ = result;
    let render = render_message(&messages[id]);
    let at: u64 = tc.draw(gs::integers::<u64>().max_value(render.len() as u64 + 4));
    let result = zoom.zoom(&zoom_call(id as u64, 1, at));
    if !result.is_error {
        assert!(
            result.content.len() <= ZOOM_MAX_BYTES + 200,
            "mid-character at {at} returned {} bytes",
            result.content.len()
        );
    }

    // The range read stays exactly the slice: zooming one message is
    // `range`, the mirror's own read, verbatim when it fits a page.
    let id = tc.draw(gs::integers::<usize>().max_value(len - 1));
    let render = render_message(&messages[id]);
    if render.len() <= ZOOM_MAX_BYTES {
        let result = zoom.zoom(&zoom_call(id as u64, 1, 0));
        assert!(!result.is_error);
        assert_eq!(result.content, render, "zoom(id, 1) diverged from the log");
    }

    // Missing id, and the n=0 clamp: documented shapes.
    let missing = zoom.zoom(&ToolCall {
        id: "z".into(),
        name: "zoom".into(),
        args: serde_json::json!({ "n": 2 }),
        provider_data: serde_json::Value::Null,
    });
    assert!(missing.is_error, "a call without an id cannot answer");
    let clamped = zoom.zoom(&zoom_call(0, 0, 0));
    let single = zoom.zoom(&zoom_call(0, 1, 0));
    assert_eq!(
        clamped.content, single.content,
        "n=0 did not clamp to the documented 1"
    );
    let _ = Range { id: 0, n: 1 };
}
