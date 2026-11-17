//! How a prompt carries a pasted payload: plain text plus a span note.
//!
//! A submitted prompt is plain text with each multi-line paste's payload
//! in place, exactly what the model reads, and the message carries a
//! [`PasteSpan`] list under [`CONSUMER`] saying which byte ranges a UI
//! should draw as chips. The engine, the providers, and the summarizer
//! never read that annotation; the UI that made the chips does, and it
//! survives a checkpoint because it rides the message.
//!
//! [`PasteSpan`] replaced an embedded encoding, [`MARK`], payload,
//! [`MARK`], written into the text; [`expand`] restored the plain form
//! at the provider seam. Sessions written that way still load:
//! [`migrate`] rewrites one into plain text plus spans. The encoding and
//! [`expand`] are kept for that migration and for nothing else; a
//! session that never carried marks is unaffected.

use serde::{Deserialize, Serialize};

use crate::{Content, Message, Role};

/// The object-replacement character: opens and closes one embedded
/// paste. Two marks always bracket a payload; a lone mark is literal
/// text.
pub const MARK: char = '\u{FFFC}';

/// One scanned piece of embedded-form text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part<'a> {
    /// Plain text between pastes; may contain newlines.
    Text(&'a str),
    /// One paste's payload, without its bracketing marks.
    Paste(&'a str),
}

/// Splits embedded-form text into its parts, left to right. A mark with
/// no partner is literal text; nothing panics on malformed input.
pub fn parts(text: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK) {
        let after = &rest[start + MARK.len_utf8()..];
        match after.find(MARK) {
            Some(end) => {
                if start > 0 {
                    parts.push(Part::Text(&rest[..start]));
                }
                parts.push(Part::Paste(&after[..end]));
                rest = &after[end + MARK.len_utf8()..];
            }
            // Unpaired: literal text up to and including the mark.
            None => {
                parts.push(Part::Text(&rest[..start + MARK.len_utf8()]));
                rest = after;
            }
        }
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest));
    }
    parts
}

/// Expands embedded pastes back to plain text, the form a provider
/// sees. Malformed marks pass through untouched.
pub fn expand(text: &str) -> String {
    let mut full = String::with_capacity(text.len());
    for part in parts(text) {
        match part {
            Part::Text(text) => full.push_str(text),
            Part::Paste(payload) => full.push_str(payload),
        }
    }
    full
}

/// Normalizes text that travels inside an embed or beside marks:
/// carriage returns to newlines, stray marks dropped, the scan must
/// stay unambiguous.
pub fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace(MARK, "")
}

/// Writes `attachments` into `input` at the [`MARK`] anchors the UI
/// placed there: each mark becomes `MARK`, its payload, [`MARK`].
/// Payloads are normalized (see [`normalize`]); a mark with no payload
/// left expands to nothing.
pub fn embed(input: &str, attachments: &[String]) -> String {
    let mut embedded = String::with_capacity(input.len());
    let mut payloads = attachments.iter();
    for (index, piece) in input.split(MARK).enumerate() {
        if index > 0
            && let Some(payload) = payloads.next()
        {
            let payload = normalize(payload);
            embedded.push(MARK);
            embedded.push_str(&payload);
            embedded.push(MARK);
        }
        embedded.push_str(piece);
    }
    embedded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_expand_round_trips() {
        let long = "l1\nl2\nl3\nl4";
        assert_eq!(
            embed(&format!("look at {MARK} this"), &[long.to_owned()]),
            format!("look at {MARK}{long}{MARK} this")
        );
        let embedded = embed(&format!("a{MARK}b"), &[long.to_owned()]);
        assert_eq!(embedded, format!("a{MARK}{long}{MARK}b"));
        assert_eq!(expand(&embedded), format!("a{long}b"));
        // Marks at both ends of the input land the same way.
        let wrapped = embed(
            &format!("{MARK}{MARK}b"),
            &["p1".to_owned(), "p2".to_owned()],
        );
        assert_eq!(wrapped, format!("{MARK}p1{MARK}{MARK}p2{MARK}b"));
        assert_eq!(expand(&wrapped), "p1p2b");
    }

    #[test]
    fn spans_round_trip_through_the_annotation() {
        let mut message = Message::text(Role::User, "look at l1\nl2 this");
        assert!(spans(&message).is_empty());
        let span = PasteSpan {
            start: 8,
            end: 13,
            lines: 2,
        };
        annotate(&mut message, &[span]);
        assert_eq!(spans(&message), vec![span]);
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["annotations"]["rhema-code"]["pastes"][0]["lines"], 2);
        // Clearing the last span drops the whole note, so an annotation-free
        // message keeps the pre-annotation wire shape.
        annotate(&mut message, &[]);
        assert!(spans(&message).is_empty());
        assert!(
            serde_json::to_value(&message)
                .unwrap()
                .get("annotations")
                .is_none()
        );
    }

    #[test]
    fn migrate_rewrites_the_legacy_form_into_plain_text_and_spans() {
        let payload = "l1\nl2\nl3\nl4";
        let mut history = vec![
            Message::text(
                Role::User,
                embed(&format!("look at {MARK} this"), &[payload.to_owned()]),
            ),
            Message::text(Role::Assistant, "on it"),
        ];
        assert_eq!(migrate(&mut history), 1);
        assert_eq!(
            history[0].content,
            Message::text(Role::User, format!("look at {payload} this")).content
        );
        assert_eq!(
            spans(&history[0]),
            vec![PasteSpan {
                start: 8,
                end: 8 + payload.len(),
                lines: 4,
            }]
        );
        // Idempotent: the plain form has no marks left to rewrite.
        assert_eq!(migrate(&mut history), 0);
        assert_eq!(history[1], Message::text(Role::Assistant, "on it"));
    }

    #[test]
    fn an_unpaired_mark_migrates_as_literal_text() {
        let mut history = vec![Message::text(Role::User, format!("a{MARK}b"))];
        assert_eq!(migrate(&mut history), 1);
        assert_eq!(
            history[0].content,
            Message::text(Role::User, "a\u{FFFC}b").content
        );
        assert!(spans(&history[0]).is_empty());
    }

    #[test]
    fn parts_scans_marks_and_payloads() {
        let embedded = format!("x{MARK}p1{MARK}y{MARK}p2{MARK}");
        assert_eq!(
            parts(&embedded),
            vec![
                Part::Text("x"),
                Part::Paste("p1"),
                Part::Text("y"),
                Part::Paste("p2"),
            ]
        );
        // No marks: the whole text is one part. Empty input: no parts.
        assert_eq!(parts("plain"), vec![Part::Text("plain")]);
        assert!(parts("").is_empty());
    }

    #[test]
    fn an_unpaired_mark_stays_literal() {
        let stray = format!("a{MARK}b");
        assert_eq!(
            parts(&stray),
            vec![Part::Text(&format!("a{MARK}")), Part::Text("b")]
        );
        assert_eq!(expand(&stray), stray);
    }

    #[test]
    fn embed_normalizes_payloads() {
        let embedded = embed(
            &format!("x{MARK}"),
            &["p\r\nq".to_owned(), format!("r{MARK}s")],
        );
        assert_eq!(embedded, format!("x{MARK}p\nq{MARK}"));
        assert_eq!(expand(&embedded), "xp\nq");
    }
}

/// The annotation key `rhema-code` records paste spans under on a user
/// message: consumer-owned, never read by the engine.
pub const CONSUMER: &str = "rhema-code";

/// One paste's span in a message's plain text: the byte range its chip
/// stands for, and the payload's line count for the chip's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteSpan {
    /// Start byte of the payload in the message text.
    pub start: usize,
    /// End byte of the payload in the message text, exclusive.
    pub end: usize,
    /// Lines in the payload, the chip's size label.
    pub lines: usize,
}

/// The annotation's own shape, `{"pastes": [...]}`.
#[derive(Serialize, Deserialize)]
struct Pastes {
    pastes: Vec<PasteSpan>,
}

/// Records `spans` on `message` under [`CONSUMER`], replacing whatever
/// the same consumer left before; an empty list clears the note.
pub fn annotate(message: &mut Message, spans: &[PasteSpan]) {
    if spans.is_empty() {
        message.annotations.remove(CONSUMER);
        return;
    }
    let value = serde_json::to_value(Pastes {
        pastes: spans.to_vec(),
    })
    .expect("paste spans serialize");
    message.annotate(CONSUMER, value);
}

/// The paste spans `message` carries, empty when it has none or when
/// the note is not this consumer's shape.
pub fn spans(message: &Message) -> Vec<PasteSpan> {
    message
        .annotation(CONSUMER)
        .and_then(|value| serde_json::from_value::<Pastes>(value.clone()).ok())
        .map(|pastes| pastes.pastes)
        .unwrap_or_default()
}

/// Splits legacy embedded text into the plain form and its spans.
fn split_legacy(text: &str) -> (String, Vec<PasteSpan>) {
    let mut plain = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK) {
        plain.push_str(&rest[..start]);
        let after = &rest[start + MARK.len_utf8()..];
        match after.find(MARK) {
            Some(end) => {
                let payload = &after[..end];
                let begin = plain.len();
                plain.push_str(payload);
                spans.push(PasteSpan {
                    start: begin,
                    end: plain.len(),
                    lines: payload.lines().count(),
                });
                rest = &after[end + MARK.len_utf8()..];
            }
            // An unpaired mark is literal text, as in [`parts`].
            None => {
                plain.push(MARK);
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    (plain, spans)
}

/// Rewrites the legacy embedded form in `history` into plain text plus
/// [`PasteSpan`] annotations, returning how many messages changed.
/// Sessions written before the annotation existed carry
/// `MARK payload MARK` inside the prompt; a resumed session must not
/// send those marks to a provider. Chips only ever lived in one user
/// text block, so only user messages are considered.
pub fn migrate(history: &mut [Message]) -> usize {
    let mut changed = 0;
    for message in history.iter_mut() {
        if message.role != Role::User {
            continue;
        }
        let mut spans = Vec::new();
        let mut touched = false;
        for content in message.content.iter_mut() {
            if let Content::Text(text) = content
                && text.contains(MARK)
            {
                let (plain, found) = split_legacy(text);
                *text = plain;
                spans.extend(found);
                touched = true;
            }
        }
        if touched {
            annotate(message, &spans);
            changed += 1;
        }
    }
    changed
}
