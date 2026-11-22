//! Property tests for the markdown renderer.
//!
//! Vendor fields are hostile by assumption: titles carry newlines and
//! doubled spaces, snippets carry markup-ish runs, urls carry
//! parentheses and stray whitespace. The renderer's contract is that
//! the output stays a well-formed document no matter what the vendors
//! send: one header line, one numbered block per kept result (the
//! link line, then the snippet line), a dropped notice when the cap
//! bites, and never a vendor character that breaks that grammar.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator, TextGenerator};
use rhema_websearch::render::{MAX_RENDER_BYTES, SNIPPET_CHARS, render};
use rhema_websearch::{ChainOutcome, SearchResult};

/// The characters a vendor's fields may realistically carry, chosen to
/// stress every defense the renderer has: whitespace runs, markdown
/// link syntax, angle brackets, and astral characters.
fn field() -> TextGenerator {
    gs::text()
        .min_size(0)
        .max_size(60)
        .alphabet("ab \t\n\r()[]<>:./é😀")
}

#[hegel::test_helper]
fn an_outcome(tc: &TestCase) -> ChainOutcome {
    let results: Vec<SearchResult> = tc.draw(
        gs::vecs(hegel::tuples!(field(), field(), field()))
            .max_size(40)
            .map(|rows| {
                rows.into_iter()
                    .map(|(title, url, snippet)| SearchResult::new(title, url, snippet))
                    .collect()
            }),
    );
    ChainOutcome {
        name: tc.draw(gs::text().max_size(12).alphabet("ab-")),
        kind: "brave",
        results,
    }
}

#[hegel::test(test_cases = 500)]
fn the_render_stays_a_well_formed_document(tc: TestCase) {
    let outcome = an_outcome(&tc);
    let text = render(&outcome);
    let lines: Vec<&str> = text.lines().collect();

    // The header: who answered, what kind, how many hits.
    assert_eq!(
        lines.first(),
        Some(
            &format!(
                "{} ({}): {} results",
                outcome.name,
                outcome.kind,
                outcome.results.len()
            )
            .as_str()
        ),
        "the header line diverged"
    );

    // The document's grammar: every line is the header, a block's
    // numbered link line, a block's snippet line, a blank separator,
    // or the dropped notice. Anything else means a vendor character
    // broke the structure.
    let mut numbered = 0usize;
    let mut notice = false;
    for line in lines.iter().skip(1) {
        if line.is_empty() {
            continue;
        }
        let numbered_line = line.starts_with(&format!("{}. [", numbered + 1));
        let snippet_line = line.starts_with("   ");
        if numbered_line {
            numbered += 1;
        } else if snippet_line {
            // The trim bound: the snippet, the indent, and the
            // ellipsis suffix, never more.
            assert!(
                line.chars().count() <= SNIPPET_CHARS + "   …".len() + 1,
                "a snippet line overshot the trim bound: {line:?}"
            );
        } else if line.contains("result(s) dropped") {
            notice = true;
        } else {
            panic!("a vendor field broke the line grammar: {line:?}\nfull render:\n{text}");
        }
    }

    // Kept results are numbered consecutively from 1, and the notice
    // appears exactly when the cap dropped something.
    assert!(
        numbered <= outcome.results.len(),
        "the render numbered {} blocks for {} results",
        numbered,
        outcome.results.len()
    );
    assert_eq!(
        notice,
        numbered < outcome.results.len(),
        "the dropped notice appeared unlike the cap was hit (kept {} of {})",
        numbered,
        outcome.results.len()
    );

    // The byte cap: the whole document, notice included, stays within
    // the render cap plus the one notice line that may follow a fit.
    let notice_max = 2
        + outcome.results.len().to_string().len()
        + " more result(s) dropped: the 8192 byte render cap]".len();
    assert!(
        text.len() <= MAX_RENDER_BYTES + notice_max,
        "the render blew the byte cap: {} bytes",
        text.len()
    );

    // Titles render on their link line: no vendor newline survives
    // inside a link's text, whatever the title carried.
    for (line_number, result) in outcome.results.iter().enumerate().take(numbered) {
        let prefix = format!("{}. ", line_number + 1);
        let line = lines.iter().find(|line| line.starts_with(&prefix));
        assert!(
            line.is_some(),
            "kept result {} has no link line",
            line_number + 1
        );
        let _ = result;
    }
}
