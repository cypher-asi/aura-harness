use super::*;
use crate::app::LinkHit;

// ============================================================================
// Links a click opens
// ============================================================================

/// The text of each span that links somewhere, with its URL.
fn linked(text: &str) -> Vec<(String, String)> {
    let parsed = parse_linked(text, base(), &theme());
    parsed
        .spans
        .iter()
        .zip(&parsed.links)
        .filter_map(|(span, link)| {
            let url = parsed.urls[(*link)?].to_string();
            Some((span.content.to_string(), url))
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(text, url)| ((*text).to_owned(), (*url).to_owned()))
        .collect()
}

#[test]
fn a_labeled_link_keeps_its_url_on_the_label_and_the_url_after_it() {
    assert_eq!(
        linked("see [the **docs**](https://example.com \"Docs\") now"),
        pairs(&[
            ("the ", "https://example.com"),
            ("docs", "https://example.com"),
            ("https://example.com", "https://example.com"),
        ])
    );
    let parsed = parse_linked("[a](https://a.dev) [b](https://a.dev)", base(), &theme());
    assert_eq!(parsed.urls.len(), 2, "two links, though to one place");
}

#[test]
fn a_bare_url_in_prose_is_a_link_without_the_punctuation_after_it() {
    let parsed = parse_linked(
        "Open https://x.dev/a, (or https://y.dev).",
        base(),
        &theme(),
    );
    let text: String = parsed.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "Open https://x.dev/a, (or https://y.dev).");
    assert_eq!(
        linked("Open https://x.dev/a, (or https://y.dev)."),
        pairs(&[
            ("https://x.dev/a", "https://x.dev/a"),
            ("https://y.dev", "https://y.dev"),
        ])
    );
    assert_eq!(
        span_style(&parsed.spans, "https://x.dev/a").fg,
        Some(theme().colors.muted)
    );
    assert_eq!(span_style(&parsed.spans, ", (or ").fg, base().fg);
}

#[test]
fn urls_in_code_and_links_elsewhere_do_not_open() {
    assert!(linked("run `curl https://x.dev` now").is_empty());
    assert!(linked("[notes](file:///etc/passwd) and [lib](src/lib.rs)").is_empty());
    assert!(linked("xhttps://x.dev").is_empty());
    // Inside a link's label a URL is the label, not a second link.
    assert_eq!(
        linked("[see https://b.dev](https://a.dev)"),
        pairs(&[
            ("see https://b.dev", "https://a.dev"),
            ("https://a.dev", "https://a.dev"),
        ])
    );
}

#[test]
fn snake_case_in_a_url_is_not_emphasis() {
    assert_eq!(
        linked("at https://x.dev/a_b_c_d now"),
        pairs(&[("https://x.dev/a_b_c_d", "https://x.dev/a_b_c_d")])
    );
}

fn hits(text: &str, width: usize) -> (Vec<String>, Vec<LinkHit>) {
    let (rendered, _) = super::super::render_markdown_linked(
        text,
        width,
        (base(), &theme()),
        Measure::GRID,
        Breaks::Lines,
    );
    (texts(&rendered.lines), rendered.links)
}

fn columns(hits: &[LinkHit]) -> Vec<(usize, u16, u16, usize)> {
    hits.iter()
        .map(|hit| (hit.line, hit.x, hit.width, hit.link))
        .collect()
}

#[test]
fn a_wrapped_link_has_a_piece_on_each_row_it_is_on() {
    let (lines, links) = hits("go [one two three](https://x.dev) end", 12);
    assert_eq!(lines, ["go one two", "three", "(https://x.d", "ev) end"]);
    assert_eq!(
        columns(&links),
        [(0, 3, 7, 0), (1, 0, 5, 0), (2, 1, 11, 0), (3, 0, 2, 0)]
    );
    assert!(links.iter().all(|hit| &*hit.url == "https://x.dev"));
}

#[test]
fn links_in_lists_and_quotes_count_their_marker_and_bar() {
    let (lines, links) = hits("- see https://a.dev\n\n> and https://b.dev", 40);
    assert_eq!(lines, ["• see https://a.dev", "", "│ and https://b.dev"]);
    assert_eq!(
        columns(&links),
        [(0, 6, 13, 0), (2, 6, 13, 1)],
        "numbered across the text"
    );
}
