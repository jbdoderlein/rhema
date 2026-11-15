//! Property tests for the shared SSE decoder.
//!
//! Transports hand the decoder byte fragments at arbitrary boundaries.
//! The contract is that the fragments' sizes never change the payloads:
//! decoding a stream whole and decoding the same bytes split anywhere
//! must yield the same events, in order, with the same error state.

use hegel::TestCase;
use hegel::generators::{self as gs, PrintableGenerator};
use rhema_provider::SseDecoder;

/// A payload the line grammar cannot split: no CR or LF, so every
/// character stays inside one `data:` line.
fn payload() -> impl PrintableGenerator<String> {
    gs::text().max_size(24).alphabet("ab01 {}[]:é😀\\")
}

/// One generated event: an optional noise line (a comment or another
/// SSE field), its data lines paired with whether the colon is
/// followed by the spec's one optional space, and whether a blank
/// line closes it.
type Event = (Option<(String, bool)>, Vec<(String, bool)>, bool);

#[hegel::test_helper]
fn generated_stream(tc: &TestCase) -> String {
    let events: Vec<Event> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::optional(hegel::tuples!(payload(), gs::booleans())),
            gs::vecs(hegel::tuples!(payload(), gs::booleans())).max_size(6),
            gs::booleans(),
        ))
        .max_size(12),
    );
    let eol: &str = tc.draw(gs::sampled_from(&["\n", "\r\n"]));

    let mut stream = String::new();
    for (noise, lines, closed) in &events {
        if let Some((noise, comment)) = noise {
            if *comment {
                stream.push_str(": ");
            } else {
                stream.push_str("event: ");
            }
            stream.push_str(noise);
            stream.push_str(eol);
        }
        for (line, spaced) in lines {
            stream.push_str("data:");
            if *spaced {
                stream.push(' ');
            }
            stream.push_str(line);
            stream.push_str(eol);
        }
        if *closed {
            stream.push_str(eol);
        }
    }
    stream
}

/// Decode `chunks` the way a transport would: push each fragment as it
/// arrives, finish the stream, and take the error if one was raised.
fn decode(chunks: &[&[u8]]) -> (Vec<String>, Option<String>) {
    let mut decoder = SseDecoder::new();
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk));
    }
    events.extend(decoder.finish());
    (events, decoder.take_error())
}

#[hegel::test]
fn arbitrary_splits_decode_to_the_same_events(tc: TestCase) {
    let whole = generated_stream(&tc);
    let (expected, expected_error) = decode(&[whole.as_bytes()]);
    assert!(
        expected_error.is_none(),
        "the generated stream poisoned the decoder: {expected_error:?}\n{whole:?}"
    );

    // Sorted byte offsets, deduplicated: every gap between neighbours is
    // one fragment, and a duplicate offset would only add an empty one.
    let mut cuts: Vec<usize> =
        tc.draw(gs::vecs(gs::integers::<usize>().min_value(0).max_value(whole.len())).max_size(24));
    cuts.sort_unstable();
    cuts.dedup();

    let mut bounds = vec![0];
    bounds.extend(cuts);
    bounds.push(whole.len());
    let bytes = whole.as_bytes();
    let chunks: Vec<&[u8]> = bounds
        .windows(2)
        .map(|pair| &bytes[pair[0]..pair[1]])
        .collect();

    let (actual, actual_error) = decode(&chunks);
    assert_eq!(
        expected, actual,
        "splitting at {bounds:?} changed the decoded events\nstream: {whole:?}"
    );
    assert_eq!(expected_error, actual_error);
}

#[hegel::test]
fn every_byte_boundary_decodes_to_the_same_events(tc: TestCase) {
    let whole = generated_stream(&tc);
    let (expected, expected_error) = decode(&[whole.as_bytes()]);
    let chunks: Vec<&[u8]> = whole.as_bytes().chunks(1).collect();
    let (actual, actual_error) = decode(&chunks);
    assert_eq!(
        expected, actual,
        "byte-at-a-time decoding changed the events\nstream: {whole:?}"
    );
    assert_eq!(expected_error, actual_error);
}
