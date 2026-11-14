//! The markdown a `search` command prints: numbered blocks, one hit
//! each, under a total cap.

use crate::chain::ChainOutcome;
use crate::result::SearchResult;

/// The most characters one snippet may carry. Snippets are a reason
/// to open the page, not the page; the vendors' own descriptions run
/// two to five sentences.
pub const SNIPPET_CHARS: usize = 300;
/// The most bytes the whole rendering may carry. Results past it drop
/// with a notice line, so the model reads the cap's name instead of
/// the shell tool's stdout cap silently eating the tail (see
/// `specs/rhema-websearch.md` §6).
pub const MAX_RENDER_BYTES: usize = 8 * 1024;

/// Renders one outcome. The header names the entry that answered and
/// the hit count, so a session whose results look wrong can be traced
/// to a provider.
pub fn render(outcome: &ChainOutcome) -> String {
    let mut out = format!(
        "{} ({}): {} results\n",
        outcome.name,
        outcome.kind,
        outcome.results.len()
    );
    if outcome.results.is_empty() {
        return out;
    }
    out.push('\n');
    let mut dropped = 0usize;
    for (index, result) in outcome.results.iter().enumerate() {
        let block = block(index + 1, result);
        if out.len() + block.len() > MAX_RENDER_BYTES {
            dropped = outcome.results.len() - index;
            break;
        }
        out.push_str(&block);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n[{dropped} more result(s) dropped: the {MAX_RENDER_BYTES} byte render cap]\n"
        ));
    }
    out
}

/// One numbered block: the link, then the snippet on its own line.
fn block(number: usize, result: &SearchResult) -> String {
    let title = escape_brackets(&result.title);
    let url = wrap_url(&result.url);
    let mut out = format!("{number}. [{title}]({url})\n");
    let snippet = collapse_whitespace(&result.snippet);
    if !snippet.is_empty() {
        out.push_str(&format!("   {}\n", trim_snippet(&snippet)));
    }
    out
}

/// Escapes the square brackets a title may carry: unescaped, they end
/// the markdown link's text at the wrong place.
fn escape_brackets(title: &str) -> String {
    title.replace('[', "\\[").replace(']', "\\]")
}

/// Wraps a url in angle brackets when its own characters would break
/// the markdown link: parentheses and spaces end a `(...)` target
/// early.
fn wrap_url(url: &str) -> String {
    if url.contains(['(', ')', ' ', '<', '>']) {
        format!("<{url}>")
    } else {
        url.to_owned()
    }
}

/// Collapses every whitespace run to one space: vendor snippets carry
/// newlines and double spaces, and a block per line is not one block.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Trims the snippet at [`SNIPPET_CHARS`], backing up to a word
/// boundary so the cut never splits a word, and marking the cut with
/// an ellipsis so the model knows the vendor text went on.
fn trim_snippet(snippet: &str) -> String {
    if snippet.chars().count() <= SNIPPET_CHARS {
        return snippet.to_owned();
    }
    let cut: String = snippet.chars().take(SNIPPET_CHARS).collect();
    match cut.rfind(' ') {
        // A word boundary exists before the cut: end there.
        Some(space) if space > 0 => format!("{} …", &cut[..space]),
        // One long word: cut mid-word rather than return all of it.
        _ => format!("{cut} …"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(results: Vec<SearchResult>) -> ChainOutcome {
        ChainOutcome {
            name: "brave-main".into(),
            kind: "brave",
            results,
        }
    }

    #[test]
    fn the_header_names_the_entry_that_answered() {
        let out = render(&outcome(vec![SearchResult::new("T", "https://x", "s")]));
        assert!(out.starts_with("brave-main (brave): 1 results\n"), "{out}");
        assert!(out.contains("1. [T](https://x)\n   s\n"), "{out}");
    }

    #[test]
    fn zero_results_is_a_success_line_not_a_blank_page() {
        assert_eq!(
            render(&outcome(Vec::new())),
            "brave-main (brave): 0 results\n"
        );
    }

    #[test]
    fn snippets_collapse_and_trim_at_a_word_boundary() {
        let long = "word ".repeat(80);
        let out = render(&outcome(vec![SearchResult::new("T", "https://x", &long)]));
        let snippet_line = out.lines().nth(3).unwrap();
        assert!(snippet_line.starts_with("   word"), "{snippet_line}");
        assert!(snippet_line.ends_with("word …"), "{snippet_line}");
        assert!(
            snippet_line.chars().count() <= SNIPPET_CHARS + 4,
            "{snippet_line}"
        );
    }

    #[test]
    fn a_snippet_of_newlines_becomes_one_line() {
        let out = render(&outcome(vec![SearchResult::new(
            "T",
            "https://x",
            "a\n\nb\t c",
        )]));
        assert!(out.contains("   a b c\n"), "{out}");
    }

    #[test]
    fn titles_keep_their_brackets_inside_the_link() {
        let out = render(&outcome(vec![SearchResult::new(
            "[RFC] notes",
            "https://x",
            "s",
        )]));
        assert!(out.contains("1. [\\[RFC\\] notes](https://x)\n"), "{out}");
    }

    #[test]
    fn urls_with_parens_wrap_in_angle_brackets() {
        let out = render(&outcome(vec![SearchResult::new(
            "T",
            "https://x/a(b)",
            "s",
        )]));
        assert!(out.contains("](<https://x/a(b)>)\n"), "{out}");
    }

    #[test]
    fn past_the_cap_results_drop_with_a_notice() {
        let filler = "x".repeat(2000);
        let results = (0..40)
            .map(|i| SearchResult::new(format!("t{i}"), format!("https://x/{i}"), &filler))
            .collect();
        let out = render(&outcome(results));
        assert!(out.len() <= MAX_RENDER_BYTES + 200, "{}", out.len());
        assert!(
            out.contains("more result(s) dropped: the 8192 byte render cap"),
            "{out}"
        );
    }
}
